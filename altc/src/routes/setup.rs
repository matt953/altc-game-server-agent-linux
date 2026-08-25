use crate::db::{passwords, tokens, users};
use crate::error::ApiError;
use crate::state::AppState;
use axum::{Json, extract::State};
use serde_json::{Value, json};

/// Public, unauthenticated, and deliberately dull: a name, a version, and
/// whether this server still needs claiming. A browser has to be able to ask
/// this before any account exists, so it must reveal nothing else.
pub async fn server_info(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({
        "name": std::env::var("ALTC_SERVER_NAME").unwrap_or_else(|_| "altc".into()),
        "version": env!("CARGO_PKG_VERSION"),
        "setup_complete": is_claimed(&s).await?,
    })))
}

/// Claimed once any account exists. Jellyfin's model: first arrival wins, and
/// the window is shut for good afterwards, so an unclaimed server on a shared
/// network cannot be quietly taken over later.
async fn is_claimed(s: &AppState) -> Result<bool, ApiError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&s.pool)
        .await?;
    Ok(count > 0)
}

#[derive(serde::Deserialize)]
pub struct Claim {
    pub name: String,
    pub password: String,
}

/// Creates the owner and closes the claim window permanently.
///
/// Unauthenticated by necessity — it is what creates the first credential.
/// The only thing standing between a stranger and this endpoint is that it
/// stops working the moment anyone uses it.
pub async fn claim(
    State(s): State<AppState>,
    Json(body): Json<Claim>,
) -> Result<Json<Value>, ApiError> {
    if is_claimed(&s).await? {
        // 409, not 403: nothing is wrong with the request, the window is
        // simply shut. Mirrors Jellyfin returning 401 on /Startup/User once
        // its wizard has been completed.
        return Err(ApiError::conflict("this server has already been set up"));
    }
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("name is required"));
    }
    passwords::validate(&body.password)?;

    let id = users::insert(&s.pool, name, "owner").await?;
    passwords::set(&s.pool, id, &body.password).await?;
    let token = tokens::issue(&s.pool, id, "setup").await?;
    let user = users::fetch(&s.pool, id).await?;

    tracing::info!("server claimed by '{name}'");
    Ok(Json(json!({ "token": token, "user": user })))
}
