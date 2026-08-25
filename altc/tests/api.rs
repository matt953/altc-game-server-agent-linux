use altc_api::{db, routes, state::AppState, wolf::WolfClient};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

async fn setup() -> (Router, String) {
    let pool = db::init_memory().await;
    let id = db::users::insert(&pool, "owner", "owner").await.unwrap();
    let token = db::tokens::issue(&pool, id, "test").await.unwrap();
    let wolf = WolfClient::new("/nonexistent/wolf.sock".into());
    (
        routes::router(AppState {
            state_dir: std::env::temp_dir().join("altc-test-state"),
            pool,
            wolf,
            events: altc_api::events::EventHub::new(),
            library: altc_api::storage::Library::default(),
            art_dir: std::env::temp_dir().join("altc-test-art"),
        }),
        token,
    )
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn get(uri: &str, token: &str) -> Request<Body> {
    Request::get(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn json_req(method: &str, uri: &str, token: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn healthz_is_public() {
    let (app, _) = setup().await;
    let (status, body) = send(&app, Request::get("/healthz").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn me_requires_token() {
    let (app, _) = setup().await;
    let (status, _) = send(
        &app,
        Request::get("/api/v1/me").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(&app, get("/api/v1/me", "altc_bogus")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn owner_can_read_me() {
    let (app, owner) = setup().await;
    let (status, body) = send(&app, get("/api/v1/me", &owner)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["role"], "owner");
}

#[tokio::test]
async fn member_cannot_use_admin_routes() {
    let (app, owner) = setup().await;
    let (status, body) = send(
        &app,
        json_req("POST", "/api/v1/users", &owner, r#"{"name":"dave"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let member = body["token"].as_str().unwrap().to_string();

    let (status, body) = send(&app, get("/api/v1/me", &member)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["role"], "member");

    let (status, _) = send(&app, get("/api/v1/users", &member)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn member_can_set_own_locale() {
    let (app, owner) = setup().await;
    let (_, body) = send(
        &app,
        json_req("POST", "/api/v1/users", &owner, r#"{"name":"dave"}"#),
    )
    .await;
    let member = body["token"].as_str().unwrap().to_string();

    let (status, body) = send(
        &app,
        json_req(
            "PATCH",
            "/api/v1/me/settings",
            &member,
            r#"{"locale":"de-DE"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["locale"], "de-DE");
}

#[tokio::test]
async fn owner_cannot_be_deleted_or_duplicated() {
    let (app, owner) = setup().await;
    let (status, _) = send(&app, json_req("DELETE", "/api/v1/users/1", &owner, "")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &app,
        json_req(
            "POST",
            "/api/v1/users",
            &owner,
            r#"{"name":"x","role":"owner"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rotate_kills_old_token() {
    let (app, owner) = setup().await;
    let (_, body) = send(
        &app,
        json_req("POST", "/api/v1/users", &owner, r#"{"name":"dave"}"#),
    )
    .await;
    let old = body["token"].as_str().unwrap().to_string();
    let id = body["user"]["id"].as_i64().unwrap();

    let (status, body) = send(
        &app,
        json_req("POST", &format!("/api/v1/users/{id}/tokens"), &owner, ""),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let new = body["token"].as_str().unwrap().to_string();

    let (status, _) = send(&app, get("/api/v1/me", &old)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(&app, get("/api/v1/me", &new)).await;
    assert_eq!(status, StatusCode::OK);
}

// Passwords are additive: a token issued before they existed must keep working
// exactly as before, or every paired device and saved client breaks.
#[tokio::test]
async fn a_token_still_works_after_passwords_exist() {
    let (app, token) = setup().await;
    let (status, _) = send(&app, get("/api/v1/me", &token)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn login_returns_a_working_token() {
    let (app, token) = setup().await;
    // The owner predates passwords, so logging in is impossible until one is set.
    let (status, _) = send(
        &app,
        Request::post("/api/v1/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"name":"owner","password":"whatever12"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &app,
        Request::put("/api/v1/me/password")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"password":"a good password"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &app,
        Request::post("/api/v1/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"name":"owner","password":"a good password"}"#,
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let issued = body["token"].as_str().unwrap().to_string();
    assert_ne!(
        issued, token,
        "login must mint a new token, not reveal the old one"
    );

    // The token login handed back must actually authenticate.
    let (status, me) = send(&app, get("/api/v1/me", &issued)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["name"], "owner");
}

#[tokio::test]
async fn a_wrong_password_and_an_unknown_user_are_indistinguishable() {
    let (app, token) = setup().await;
    send(
        &app,
        Request::put("/api/v1/me/password")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"password":"a good password"}"#))
            .unwrap(),
    )
    .await;

    let attempt = |body: &'static str| {
        let app = app.clone();
        async move {
            send(
                &app,
                Request::post("/api/v1/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
        }
    };
    let (s1, b1) = attempt(r#"{"name":"owner","password":"wrong"}"#).await;
    let (s2, b2) = attempt(r#"{"name":"ghost","password":"wrong"}"#).await;
    assert_eq!(s1, StatusCode::UNAUTHORIZED);
    assert_eq!(s2, StatusCode::UNAUTHORIZED);
    // Same status AND same body, or the difference leaks which names exist.
    assert_eq!(b1, b2);
}

#[tokio::test]
async fn a_short_password_is_refused() {
    let (app, token) = setup().await;
    let (status, _) = send(
        &app,
        Request::put("/api/v1/me/password")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"password":"short"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
