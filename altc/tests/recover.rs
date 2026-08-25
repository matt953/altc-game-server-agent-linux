use altc_api::{db, routes, state::AppState, wolf::WolfClient};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

/// A locked-out server: an owner exists with a password nobody remembers.
async fn locked_out(test: &str) -> (Router, sqlx::SqlitePool, std::path::PathBuf) {
    let state_dir =
        std::env::temp_dir().join(format!("altc-recover-{}-{test}", std::process::id()));
    std::fs::remove_dir_all(&state_dir).ok();
    std::fs::create_dir_all(&state_dir).unwrap();

    let pool = db::init_memory().await;
    let id = db::users::insert(&pool, "matt", "owner").await.unwrap();
    db::passwords::set(&pool, id, "the forgotten one")
        .await
        .unwrap();

    let app = routes::router(AppState {
        state_dir: state_dir.clone(),
        pool: pool.clone(),
        wolf: WolfClient::new("/nonexistent/wolf.sock".into()),
        events: altc_api::events::EventHub::new(),
        library: altc_api::storage::Library::default(),
        art_dir: std::env::temp_dir().join("altc-test-art"),
    });
    (app, pool, state_dir)
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
async fn recovery_is_refused_without_the_file() {
    let (app, pool, dir) = locked_out("no_file").await;
    let (status, body) = send(
        &app,
        post(
            "/api/v1/recover",
            r#"{"name":"matt","password":"a new password"}"#,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "anyone on the network could reset otherwise"
    );
    // The error tells you what to do, since the person reading it is locked out.
    assert!(
        body["error"].as_str().unwrap().contains("recover"),
        "got {body}"
    );

    // The old password must still be the only one that works.
    assert!(
        db::passwords::check(&pool, "matt", "a new password")
            .await
            .is_none()
    );
    assert!(
        db::passwords::check(&pool, "matt", "the forgotten one")
            .await
            .is_some()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_file_authorises_a_reset_and_is_then_consumed() {
    let (app, pool, dir) = locked_out("with_file").await;
    std::fs::write(dir.join("recover"), b"").unwrap();

    let (status, body) = send(
        &app,
        post(
            "/api/v1/recover",
            r#"{"name":"matt","password":"a new password"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user"]["name"], "matt");

    // The new password works and the old one does not.
    assert!(
        db::passwords::check(&pool, "matt", "a new password")
            .await
            .is_some()
    );
    assert!(
        db::passwords::check(&pool, "matt", "the forgotten one")
            .await
            .is_none()
    );

    // The token it returns must actually authenticate, or you are still locked out.
    let token = body["token"].as_str().unwrap();
    let (status, _) = send(
        &app,
        Request::get("/api/v1/me")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Consumed, so recovery is not left standing open.
    assert!(!dir.join("recover").exists());
    let (status, _) = send(
        &app,
        post(
            "/api/v1/recover",
            r#"{"name":"matt","password":"again please"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "one file, one reset");
    std::fs::remove_dir_all(&dir).ok();
}

// The whole point of recovery rather than starting over.
#[tokio::test]
async fn a_reset_destroys_nothing_but_the_password() {
    let (app, pool, dir) = locked_out("keeps_data").await;
    std::fs::write(dir.join("recover"), b"").unwrap();

    let dave = db::users::insert(&pool, "dave", "member").await.unwrap();
    db::devices::upsert(&pool, dave, "cert-hash-1", &Some("daves-tv".to_string()))
        .await
        .unwrap();
    db::shares::set_for_app(&pool, "578802895", &[dave])
        .await
        .unwrap();

    send(
        &app,
        post(
            "/api/v1/recover",
            r#"{"name":"matt","password":"a new password"}"#,
        ),
    )
    .await;

    // Accounts, devices and shares all survive.
    assert_eq!(db::users::list(&pool).await.unwrap().len(), 2);
    assert_eq!(
        db::devices::list_for_user(&pool, dave).await.unwrap().len(),
        1
    );
    assert_eq!(
        db::shares::for_app(&pool, "578802895").await.unwrap(),
        vec![dave]
    );
    // And dave's own password is untouched.
    assert!(!db::passwords::has_password(&pool, dave).await);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_failed_attempt_leaves_recovery_armed() {
    let (app, _pool, dir) = locked_out("still_armed").await;
    std::fs::write(dir.join("recover"), b"").unwrap();

    // Too short: rejected, and the one chance must not be spent on a typo.
    let (status, _) = send(
        &app,
        post("/api/v1/recover", r#"{"name":"matt","password":"short"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        dir.join("recover").exists(),
        "a typo must not lock you out again"
    );

    // Wrong account name: same.
    let (status, _) = send(
        &app,
        post(
            "/api/v1/recover",
            r#"{"name":"nobody","password":"a new password"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(dir.join("recover").exists());
    std::fs::remove_dir_all(&dir).ok();
}
