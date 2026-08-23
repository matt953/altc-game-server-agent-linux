use altc_api::{db, routes, state::AppState, wolf::WolfClient};
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode, header},
    routing::{get, post},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct FakeWolf {
    unpaired: Arc<Mutex<Vec<String>>>,
}

fn fake_wolf_router(state: FakeWolf) -> Router {
    Router::new()
        .route(
            "/api/v1/pair/pending",
            get(|| async {
                Json(json!({"success": true, "requests": [
                    {"pair_secret": "s3cret", "client_ip": "10.0.0.5", "client_id": "cert-hash-1"}
                ]}))
            }),
        )
        .route(
            "/api/v1/pair/client",
            post(|Json(v): Json<Value>| async move {
                if v["pair_secret"] == "s3cret" && v["pin"] == "1234" {
                    Json(json!({"success": true, "client_id": "cert-hash-1"}))
                } else {
                    Json(json!({"success": false, "error": "invalid pair secret"}))
                }
            }),
        )
        .route(
            "/api/v1/clients",
            get(|| async {
                Json(json!({"success": true, "clients": [
                    {"client_id": "cert-hash-1", "app_state_folder": "cert-hash-1"},
                    {"client_id": "legacy-cert", "app_state_folder": "legacy-cert"}
                ]}))
            }),
        )
        .route(
            "/api/v1/unpair/client",
            post(
                |State(fw): State<FakeWolf>, Json(v): Json<Value>| async move {
                    fw.unpaired
                        .lock()
                        .unwrap()
                        .push(v["client_id"].as_str().unwrap().to_string());
                    Json(json!({"success": true}))
                },
            ),
        )
        .with_state(state)
}

async fn setup(test: &str) -> (Router, String, FakeWolf, sqlx::SqlitePool) {
    let socket = PathBuf::from(std::env::temp_dir())
        .join(format!("altc-fw-{}-{test}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let fake = FakeWolf::default();
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let fw = fake.clone();
    tokio::spawn(async move {
        axum::serve(listener, fake_wolf_router(fw)).await.unwrap();
    });

    let pool = db::init_memory().await;
    let id = db::users::insert(&pool, "owner", "owner").await.unwrap();
    let token = db::tokens::issue(&pool, id, "test").await.unwrap();
    let wolf = WolfClient::new(socket);
    let app = routes::router(AppState {
        pool: pool.clone(),
        wolf,
        events: altc_api::events::EventHub::new(),
    });
    (app, token, fake, pool)
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

async fn create_member(app: &Router, owner: &str, name: &str) -> (i64, String) {
    let (_, body) = send(
        app,
        authed(
            "POST",
            "/api/v1/users",
            owner,
            &format!(r#"{{"name":"{name}"}}"#),
        ),
    )
    .await;
    (
        body["user"]["id"].as_i64().unwrap(),
        body["token"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
async fn pending_pairs_include_client_id() {
    let (app, owner, _, _) = setup("pending").await;
    let (status, body) = send(&app, authed("GET", "/api/v1/pair/pending", &owner, "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["pair_secret"], "s3cret");
    assert_eq!(body[0]["client_id"], "cert-hash-1");
}

#[tokio::test]
async fn member_self_approval_binds_device() {
    let (app, owner, _, _) = setup("selfpair").await;
    let (_, member) = create_member(&app, &owner, "dave").await;

    let (status, body) = send(
        &app,
        authed(
            "POST",
            "/api/v1/pair",
            &member,
            r#"{"pair_secret":"s3cret","pin":"1234","device_name":"daves-phone"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["device"]["client_id"], "cert-hash-1");

    let (status, body) = send(&app, authed("GET", "/api/v1/me/devices", &member, "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["client_id"], "cert-hash-1");
    assert_eq!(body[0]["name"], "daves-phone");

    let (_, body) = send(&app, authed("GET", "/api/v1/me/devices", &owner, "")).await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn member_cannot_pair_for_someone_else() {
    let (app, owner, _, _) = setup("proxypair").await;
    let (_, member) = create_member(&app, &owner, "dave").await;

    let (status, _) = send(
        &app,
        authed(
            "POST",
            "/api/v1/pair",
            &member,
            r#"{"pair_secret":"s3cret","pin":"1234","user_id":1}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_pairs_on_behalf_of_user() {
    let (app, owner, _, _) = setup("behalf").await;
    let (dave_id, dave) = create_member(&app, &owner, "dave").await;

    let (status, body) = send(
        &app,
        authed(
            "POST",
            "/api/v1/pair",
            &owner,
            &format!(r#"{{"pair_secret":"s3cret","pin":"1234","user_id":{dave_id}}}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["device"]["user_id"], dave_id);

    let (_, body) = send(&app, authed("GET", "/api/v1/me/devices", &dave, "")).await;
    assert_eq!(body[0]["client_id"], "cert-hash-1");
}

#[tokio::test]
async fn admin_claims_legacy_device() {
    let (app, owner, _, _) = setup("claim").await;
    let (dave_id, dave) = create_member(&app, &owner, "dave").await;

    let (status, body) = send(
        &app,
        authed(
            "POST",
            "/api/v1/devices/legacy-cert/claim",
            &owner,
            &format!(r#"{{"user_id":{dave_id},"device_name":"tv"}}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user_id"], dave_id);

    let (status, _) = send(
        &app,
        authed(
            "POST",
            "/api/v1/devices/not-a-real-cert/claim",
            &owner,
            &format!(r#"{{"user_id":{dave_id}}}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, body) = send(&app, authed("GET", "/api/v1/me/devices", &dave, "")).await;
    assert_eq!(body[0]["name"], "tv");
}

#[tokio::test]
async fn deleting_user_unpairs_their_devices() {
    let (app, owner, fake, pool) = setup("revoke").await;
    let (dave_id, dave) = create_member(&app, &owner, "dave").await;

    send(
        &app,
        authed(
            "POST",
            "/api/v1/pair",
            &dave,
            r#"{"pair_secret":"s3cret","pin":"1234"}"#,
        ),
    )
    .await;

    let (status, _) = send(
        &app,
        authed("DELETE", &format!("/api/v1/users/{dave_id}"), &owner, ""),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(*fake.unpaired.lock().unwrap(), vec!["cert-hash-1"]);
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn unpair_clears_device_row() {
    let (app, owner, fake, pool) = setup("unpair").await;
    send(
        &app,
        authed(
            "POST",
            "/api/v1/pair",
            &owner,
            r#"{"pair_secret":"s3cret","pin":"1234"}"#,
        ),
    )
    .await;

    let (status, _) = send(
        &app,
        authed(
            "POST",
            "/api/v1/unpair",
            &owner,
            r#"{"client_id":"cert-hash-1"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(*fake.unpaired.lock().unwrap(), vec!["cert-hash-1"]);
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn pairing_requires_auth_but_not_admin() {
    let (app, owner, _, _) = setup("authz").await;
    let (_, member) = create_member(&app, &owner, "dave").await;

    let (status, _) = send(
        &app,
        Request::post("/api/v1/pair")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"pair_secret":"s3cret","pin":"1234"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Members see pending pairs (needed for self-approval) but not the admin client list.
    let (status, _) = send(&app, authed("GET", "/api/v1/pair/pending", &member, "")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app, authed("GET", "/api/v1/clients", &member, "")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &app,
        Request::get("/api/v1/pair/pending")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
