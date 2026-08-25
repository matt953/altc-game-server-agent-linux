use altc_api::{db, routes, state::AppState, wolf::WolfClient};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

/// A server nobody has claimed yet: no users at all, which is what a fresh
/// install now looks like since the owner is no longer auto-created.
async fn unclaimed() -> (Router, sqlx::SqlitePool) {
    let pool = db::init_memory().await;
    let app = routes::router(AppState {
        pool: pool.clone(),
        wolf: WolfClient::new("/nonexistent/wolf.sock".into()),
        events: altc_api::events::EventHub::new(),
        library: altc_api::storage::Library::default(),
        art_dir: std::env::temp_dir().join("altc-test-art"),
    });
    (app, pool)
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

fn post(uri: &str, body: &str) -> Request<Body> {
    Request::post(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn an_unclaimed_server_says_so_without_authentication() {
    let (app, _) = unclaimed().await;
    let (status, body) = send(
        &app,
        Request::get("/api/v1/server").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a browser must be able to ask before any account exists"
    );
    assert_eq!(body["setup_complete"], false);
    assert!(body["version"].is_string());
    // Nothing else may leak from a public endpoint.
    assert!(body.get("token").is_none() && body.get("users").is_none());
}

#[tokio::test]
async fn claiming_creates_the_owner_and_shuts_the_window() {
    let (app, pool) = unclaimed().await;

    let (status, body) = send(
        &app,
        post(
            "/api/v1/setup",
            r#"{"name":"matt","password":"a good password"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED.min(StatusCode::OK));
    let token = body["token"].as_str().unwrap().to_string();
    assert_eq!(body["user"]["role"], "owner");
    assert_eq!(body["user"]["name"], "matt");

    // The token it handed back must actually work.
    let (status, me) = send(
        &app,
        Request::get("/api/v1/me")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["name"], "matt");

    // And the password works, so the owner can sign in again later.
    let (status, _) = send(
        &app,
        post(
            "/api/v1/login",
            r#"{"name":"matt","password":"a good password"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The window is now shut, publicly and permanently.
    let (_, info) = send(
        &app,
        Request::get("/api/v1/server").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(info["setup_complete"], true);
    let (status, _) = send(
        &app,
        post(
            "/api/v1/setup",
            r#"{"name":"attacker","password":"another password"}"#,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a claimed server must not be re-claimable"
    );
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        users, 1,
        "the second attempt must not have created anything"
    );
}

// Matt's server is ALREADY claimed, so this is the path his deployment takes.
// Testing only the happy path on a fresh database would miss it entirely.
#[tokio::test]
async fn an_already_claimed_server_refuses_setup_from_the_start() {
    let pool = db::init_memory().await;
    db::users::insert(&pool, "owner", "owner").await.unwrap();
    let app = routes::router(AppState {
        pool: pool.clone(),
        wolf: WolfClient::new("/nonexistent/wolf.sock".into()),
        events: altc_api::events::EventHub::new(),
        library: altc_api::storage::Library::default(),
        art_dir: std::env::temp_dir().join("altc-test-art"),
    });

    let (_, info) = send(
        &app,
        Request::get("/api/v1/server").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(info["setup_complete"], true);

    let (status, _) = send(
        &app,
        post(
            "/api/v1/setup",
            r#"{"name":"someone","password":"a good password"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_weak_or_nameless_claim_is_refused_and_leaves_it_unclaimed() {
    let (app, pool) = unclaimed().await;

    for body in [
        r#"{"name":"matt","password":"short"}"#,
        r#"{"name":"","password":"a good password"}"#,
        r#"{"name":"   ","password":"a good password"}"#,
    ] {
        let (status, _) = send(&app, post("/api/v1/setup", body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "for {body}");
    }

    // A rejected claim must not consume the one chance to claim.
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(users, 0);
    let (_, info) = send(
        &app,
        Request::get("/api/v1/server").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(info["setup_complete"], false);
}
