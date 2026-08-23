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
    (routes::router(AppState { pool, wolf }), token)
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

    assert_eq!(body[0]["id"], "134906179");
    assert_eq!(body[0]["title"], "Wolf UI");
    assert_eq!(body[0]["support_hdr"], false);
    assert_eq!(body[0]["icon"], "https://example.com/wolf_ui_icon.png");

    assert_eq!(body[1]["title"], "Baldur’s Gate 3");
    assert_eq!(body[1]["icon"], Value::Null);

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
