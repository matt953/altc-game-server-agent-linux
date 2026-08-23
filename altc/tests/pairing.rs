use altc_api::{db, routes, state::AppState, wolf::WolfClient};
use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::{get, post},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::path::PathBuf;
use tower::ServiceExt;

fn fake_wolf_router() -> Router {
    Router::new()
        .route(
            "/api/v1/pair/pending",
            get(|| async {
                Json(json!({"success": true, "requests": [
                    {"pair_secret": "s3cret", "client_ip": "10.0.0.5"}
                ]}))
            }),
        )
        .route(
            "/api/v1/pair/client",
            post(|Json(v): Json<Value>| async move {
                if v["pair_secret"] == "s3cret" && v["pin"] == "1234" {
                    Json(json!({"success": true}))
                } else {
                    Json(json!({"success": false, "error": "invalid pair secret"}))
                }
            }),
        )
        .route(
            "/api/v1/clients",
            get(|| async {
                Json(json!({"success": true, "clients": [
                    {"client_id": "abc123", "app_state_folder": "/etc/wolf/abc123"}
                ]}))
            }),
        )
        .route(
            "/api/v1/unpair/client",
            post(|Json(v): Json<Value>| async move {
                if v["client_id"] == "abc123" {
                    Json(json!({"success": true}))
                } else {
                    Json(json!({"success": false, "error": "unknown client"}))
                }
            }),
        )
}

async fn setup(test: &str) -> (Router, String) {
    let socket = PathBuf::from(std::env::temp_dir())
        .join(format!("altc-fake-wolf-{}-{test}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    tokio::spawn(async move {
        axum::serve(listener, fake_wolf_router()).await.unwrap();
    });

    let pool = db::init_memory().await;
    let id = db::users::insert(&pool, "owner", "owner").await.unwrap();
    let token = db::tokens::issue(&pool, id, "test").await.unwrap();
    let wolf = WolfClient::new(socket);
    (routes::router(AppState { pool, wolf }), token)
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn authed(method: &str, uri: &str, token: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn pending_pairs_proxied_from_wolf() {
    let (app, owner) = setup("pending").await;
    let (status, body) = send(&app, authed("GET", "/api/v1/pair/pending", &owner, "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["pair_secret"], "s3cret");
    assert_eq!(body[0]["client_ip"], "10.0.0.5");
}

#[tokio::test]
async fn approve_pair_with_pin() {
    let (app, owner) = setup("approve").await;
    let (status, body) = send(
        &app,
        authed(
            "POST",
            "/api/v1/pair",
            &owner,
            r#"{"pair_secret":"s3cret","pin":"1234"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);

    let (status, body) = send(
        &app,
        authed(
            "POST",
            "/api/v1/pair",
            &owner,
            r#"{"pair_secret":"wrong","pin":"1234"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("invalid pair secret")
    );
}

#[tokio::test]
async fn clients_and_unpair() {
    let (app, owner) = setup("clients").await;
    let (status, body) = send(&app, authed("GET", "/api/v1/clients", &owner, "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["client_id"], "abc123");

    let (status, body) = send(
        &app,
        authed(
            "POST",
            "/api/v1/unpair",
            &owner,
            r#"{"client_id":"abc123"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
}

#[tokio::test]
async fn wolf_down_maps_to_bad_gateway() {
    let pool = db::init_memory().await;
    let id = db::users::insert(&pool, "owner", "owner").await.unwrap();
    let token = db::tokens::issue(&pool, id, "test").await.unwrap();
    let wolf = WolfClient::new("/nonexistent/wolf.sock".into());
    let app = routes::router(AppState { pool, wolf });

    let (status, _) = send(&app, authed("GET", "/api/v1/pair/pending", &token, "")).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn pairing_requires_admin() {
    let (app, owner) = setup("member").await;
    let (_, body) = send(
        &app,
        authed("POST", "/api/v1/users", &owner, r#"{"name":"dave"}"#),
    )
    .await;
    let member = body["token"].as_str().unwrap().to_string();

    let (status, _) = send(&app, authed("GET", "/api/v1/pair/pending", &member, "")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
