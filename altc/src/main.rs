mod auth;
mod db;

use auth::{AdminUser, AuthedUser};
use axum::{
    Json, Router,
    extract::{Path as UrlPath, State},
    http::StatusCode,
    routing::{delete, get, patch, post},
};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();

    let state_dir = PathBuf::from(
        std::env::var("HOST_APPS_STATE_FOLDER").unwrap_or_else(|_| "/etc/wolf".into()),
    )
    .join("altc");
    fs::create_dir_all(&state_dir).expect("create altc state dir");

    let pool = db::init(&state_dir).await;
    db::bootstrap_owner(&pool).await;
    let state = AppState { pool };

    let (cert, key) = ensure_tls_cert(&state_dir);
    let port: u16 = std::env::var("ALTC_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(47990);

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/me", get(me))
        .route("/api/v1/me/settings", patch(update_my_settings))
        .route("/api/v1/users", get(list_users).post(create_user))
        .route("/api/v1/users/{id}", delete(delete_user))
        .route("/api/v1/users/{id}/tokens", post(rotate_token))
        .with_state(state);

    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
        .await
        .expect("load tls cert");
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("altc-api listening on https://{addr}");
    axum_server::bind_rustls(addr, tls)
        .serve(app.into_make_service())
        .await
        .expect("server");
}

async fn healthz() -> Json<Value> {
    Json(json!({"status": "ok", "service": "altc-api", "version": env!("CARGO_PKG_VERSION")}))
}

async fn me(user: AuthedUser) -> Json<AuthedUser> {
    Json(user)
}

#[derive(serde::Deserialize)]
struct SettingsPatch {
    locale: Option<String>,
}

async fn update_my_settings(
    State(s): State<AppState>,
    user: AuthedUser,
    Json(p): Json<SettingsPatch>,
) -> Result<Json<AuthedUser>, (StatusCode, String)> {
    sqlx::query("UPDATE users SET locale = ? WHERE id = ?")
        .bind(&p.locale)
        .bind(user.id)
        .execute(&s.pool)
        .await
        .map_err(internal)?;
    let updated = fetch_user(&s.pool, user.id).await.map_err(internal)?;
    Ok(Json(updated))
}

async fn list_users(
    State(s): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<AuthedUser>>, (StatusCode, String)> {
    let users =
        sqlx::query_as::<_, AuthedUser>("SELECT id, name, role, locale FROM users ORDER BY id")
            .fetch_all(&s.pool)
            .await
            .map_err(internal)?;
    Ok(Json(users))
}

#[derive(serde::Deserialize)]
struct NewUser {
    name: String,
    role: Option<String>,
}

async fn create_user(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Json(n): Json<NewUser>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, String)> {
    let role = n.role.unwrap_or_else(|| "member".into());
    if role == "owner" {
        return Err((StatusCode::BAD_REQUEST, "owner is unique".into()));
    }
    if !["admin", "member"].contains(&role.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            "role must be admin or member".into(),
        ));
    }
    let id: i64 = sqlx::query_scalar("INSERT INTO users (name, role) VALUES (?, ?) RETURNING id")
        .bind(&n.name)
        .bind(&role)
        .fetch_one(&s.pool)
        .await
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;
    let token = issue_token(&s.pool, id, "initial")
        .await
        .map_err(internal)?;
    let user = fetch_user(&s.pool, id).await.map_err(internal)?;
    tracing::info!("user '{}' ({role}) created by '{}'", n.name, actor.name);
    Ok((
        StatusCode::CREATED,
        Json(json!({"user": user, "token": token})),
    ))
}

async fn delete_user(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    UrlPath(id): UrlPath<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    let target = fetch_user(&s.pool, id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "no such user".into()))?;
    if target.role == "owner" {
        return Err((StatusCode::BAD_REQUEST, "owner cannot be deleted".into()));
    }
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(&s.pool)
        .await
        .map_err(internal)?;
    tracing::info!("user '{}' deleted by '{}'", target.name, actor.name);
    Ok(StatusCode::NO_CONTENT)
}

async fn rotate_token(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    UrlPath(id): UrlPath<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let target = fetch_user(&s.pool, id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "no such user".into()))?;
    sqlx::query("DELETE FROM tokens WHERE user_id = ?")
        .bind(id)
        .execute(&s.pool)
        .await
        .map_err(internal)?;
    let token = issue_token(&s.pool, id, "rotated")
        .await
        .map_err(internal)?;
    tracing::info!("token rotated for '{}' by '{}'", target.name, actor.name);
    Ok(Json(json!({"token": token})))
}

async fn issue_token(pool: &SqlitePool, user_id: i64, label: &str) -> Result<String, sqlx::Error> {
    let token = auth::new_token();
    sqlx::query("INSERT INTO tokens (user_id, token_hash, label) VALUES (?, ?, ?)")
        .bind(user_id)
        .bind(auth::hash_token(&token))
        .bind(label)
        .execute(pool)
        .await?;
    Ok(token)
}

async fn fetch_user(pool: &SqlitePool, id: i64) -> Result<AuthedUser, sqlx::Error> {
    sqlx::query_as::<_, AuthedUser>("SELECT id, name, role, locale FROM users WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

// Self-signed cert generated once, persisted so clients can trust-on-first-use.
fn ensure_tls_cert(dir: &Path) -> (PathBuf, PathBuf) {
    let cert_path = dir.join("api-cert.pem");
    let key_path = dir.join("api-key.pem");
    if !cert_path.exists() || !key_path.exists() {
        let ck = rcgen::generate_simple_self_signed(vec!["altc".into()])
            .expect("generate self-signed cert");
        fs::write(&cert_path, ck.cert.pem()).expect("write cert");
        fs::write(&key_path, ck.key_pair.serialize_pem()).expect("write key");
    }
    (cert_path, key_path)
}
