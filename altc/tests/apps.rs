use altc_api::{db, routes, state::AppState, wolf::WolfClient};
use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::get,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::path::PathBuf;
use tower::ServiceExt;

// Mirrors the exact shape captured from production Wolf 2026-08-23.
fn fake_wolf_router() -> Router {
    Router::new().route(
        "/api/v1/apps",
        get(|| async {
            Json(json!({"success": true, "apps": [
                {"title": "Wolf UI", "id": "134906179", "support_hdr": false,
                 "icon_png_path": "https://example.com/wolf_ui_icon.png",
                 "h264_gst_pipeline": "interpipesrc name=... vah264enc ...",
                 "hevc_gst_pipeline": "...", "av1_gst_pipeline": "...",
                 "render_node": "/dev/dri/renderD128",
                 "opus_gst_pipeline": "...", "start_virtual_compositor": true,
                 "start_audio_server": true, "runner": {"type": "docker"}},
                {"title": "Baldur’s Gate 3", "id": "1354165435", "support_hdr": false,
                 "icon_png_path": "",
                 "h264_gst_pipeline": "...", "hevc_gst_pipeline": "...",
                 "av1_gst_pipeline": "...", "render_node": "/dev/dri/renderD128",
                 "opus_gst_pipeline": "...", "start_virtual_compositor": true,
                 "start_audio_server": true, "runner": {"type": "docker"}}
            ]}))
        }),
    )
}

async fn setup(test: &str) -> (Router, String) {
    let socket = PathBuf::from(std::env::temp_dir())
        .join(format!("altc-apps-{}-{test}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    tokio::spawn(async move {
        axum::serve(listener, fake_wolf_router()).await.unwrap();
    });
    let pool = db::init_memory().await;
    let id = db::users::insert(&pool, "owner", "owner").await.unwrap();
    let token = db::tokens::issue(&pool, id, "test").await.unwrap();
    let wolf = WolfClient::new(socket);
    // The library is adopted from Wolf once, then owned by us: exercise the
    // real import rather than seeding rows behind its back.
    altc_api::library::import_if_empty(&pool, &wolf)
        .await
        .unwrap();
    (
        routes::router(AppState {
            pool,
            wolf,
            events: altc_api::events::EventHub::new(),
        }),
        token,
    )
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

async fn make_member(app: &Router, owner: &str, name: &str) -> (i64, String) {
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

fn titles(body: &Value) -> Vec<String> {
    body.as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn apps_default_to_everyone() {
    let (app, owner) = setup("shares_default").await;
    let (_, member) = make_member(&app, &owner, "dave").await;
    let (status, body) = send(&app, authed("GET", "/api/v1/apps", &member, "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(titles(&body).len(), 2);

    let (_, body) = send(
        &app,
        authed("GET", "/api/v1/apps/134906179/shares", &owner, ""),
    )
    .await;
    assert_eq!(body["everyone"], true);
    assert_eq!(body["user_ids"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn restricting_an_app_hides_it_from_others() {
    let (app, owner) = setup("shares_restrict").await;
    let (dave_id, dave) = make_member(&app, &owner, "dave").await;
    let (_, erin) = make_member(&app, &owner, "erin").await;

    let (status, body) = send(
        &app,
        authed(
            "PUT",
            "/api/v1/apps/1354165435/shares",
            &owner,
            &format!(r#"{{"user_ids":[{dave_id}]}}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["everyone"], false);

    // Dave keeps it, Erin loses it, admin still sees everything.
    let (_, body) = send(&app, authed("GET", "/api/v1/apps", &dave, "")).await;
    assert!(titles(&body).contains(&"Baldur’s Gate 3".to_string()));
    let (_, body) = send(&app, authed("GET", "/api/v1/apps", &erin, "")).await;
    assert_eq!(titles(&body), vec!["Wolf UI".to_string()]);
    let (_, body) = send(&app, authed("GET", "/api/v1/apps", &owner, "")).await;
    assert_eq!(titles(&body).len(), 2);
}

#[tokio::test]
async fn clearing_shares_restores_everyone() {
    let (app, owner) = setup("shares_clear").await;
    let (dave_id, _) = make_member(&app, &owner, "dave").await;
    let (_, erin) = make_member(&app, &owner, "erin").await;

    send(
        &app,
        authed(
            "PUT",
            "/api/v1/apps/1354165435/shares",
            &owner,
            &format!(r#"{{"user_ids":[{dave_id}]}}"#),
        ),
    )
    .await;
    let (_, body) = send(&app, authed("GET", "/api/v1/apps", &erin, "")).await;
    assert_eq!(titles(&body).len(), 1);

    send(
        &app,
        authed(
            "PUT",
            "/api/v1/apps/1354165435/shares",
            &owner,
            r#"{"user_ids":[]}"#,
        ),
    )
    .await;
    let (_, body) = send(&app, authed("GET", "/api/v1/apps", &erin, "")).await;
    assert_eq!(titles(&body).len(), 2);
}

#[tokio::test]
async fn share_edits_are_admin_only_and_validated() {
    let (app, owner) = setup("shares_authz").await;
    let (dave_id, dave) = make_member(&app, &owner, "dave").await;

    let (status, _) = send(
        &app,
        authed(
            "PUT",
            "/api/v1/apps/1354165435/shares",
            &dave,
            &format!(r#"{{"user_ids":[{dave_id}]}}"#),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &app,
        authed("GET", "/api/v1/apps/1354165435/shares", &dave, ""),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &app,
        authed(
            "PUT",
            "/api/v1/apps/not-an-app/shares",
            &owner,
            r#"{"user_ids":[]}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &app,
        authed(
            "PUT",
            "/api/v1/apps/1354165435/shares",
            &owner,
            r#"{"user_ids":[9999]}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deleting_user_drops_their_shares() {
    let (app, owner) = setup("shares_cascade").await;
    let (dave_id, _) = make_member(&app, &owner, "dave").await;
    send(
        &app,
        authed(
            "PUT",
            "/api/v1/apps/1354165435/shares",
            &owner,
            &format!(r#"{{"user_ids":[{dave_id}]}}"#),
        ),
    )
    .await;
    send(
        &app,
        authed("DELETE", &format!("/api/v1/users/{dave_id}"), &owner, ""),
    )
    .await;

    let (_, body) = send(
        &app,
        authed("GET", "/api/v1/apps/1354165435/shares", &owner, ""),
    )
    .await;
    assert_eq!(body["everyone"], true);
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn apps_trimmed_for_clients() {
    let (app, token) = setup("trim").await;
    let (status, body) = send(
        &app,
        Request::get("/api/v1/apps")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The library is ordered by title, not by whatever order Wolf held it in.
    assert_eq!(titles(&body), vec!["Baldur\u{2019}s Gate 3", "Wolf UI"]);

    let by_id = |id: &str| {
        body.as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == id)
            .unwrap()
            .clone()
    };
    let ui = by_id("134906179");
    assert_eq!(ui["title"], "Wolf UI");
    assert_eq!(ui["support_hdr"], false);
    assert_eq!(ui["icon"], "https://example.com/wolf_ui_icon.png");

    let bg3 = by_id("1354165435");
    assert_eq!(bg3["title"], "Baldur\u{2019}s Gate 3");
    assert_eq!(bg3["icon"], Value::Null);

    let raw = body.to_string();
    assert!(!raw.contains("pipeline"));
    assert!(!raw.contains("render_node"));
    assert!(!raw.contains("runner"));
}

#[tokio::test]
async fn apps_require_auth() {
    let (app, _) = setup("auth").await;
    let (status, _) = send(
        &app,
        Request::get("/api/v1/apps").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn shares_for_an_unknown_app_are_a_404_not_everyone() {
    let (app, owner) = setup("shares_unknown").await;
    let (status, _) = send(
        &app,
        authed("GET", "/api/v1/apps/does-not-exist/shares", &owner, ""),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
