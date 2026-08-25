use crate::auth::AdminUser;
use crate::db::{devices, tokens, users, users::User};
use crate::error::ApiError;
use crate::state::AppState;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde_json::{Value, json};

pub async fn list(
    State(s): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<User>>, ApiError> {
    Ok(Json(users::list(&s.pool).await?))
}

#[derive(serde::Deserialize)]
pub struct NewUser {
    name: String,
    role: Option<String>,
}

pub async fn create(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Json(n): Json<NewUser>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let role = n.role.unwrap_or_else(|| "member".into());
    if role == "owner" {
        return Err(ApiError::bad_request("owner is unique"));
    }
    if !["admin", "member"].contains(&role.as_str()) {
        return Err(ApiError::bad_request("role must be admin or member"));
    }
    let id = users::insert(&s.pool, &n.name, &role)
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    let token = tokens::issue(&s.pool, id, "initial").await?;
    let user = users::fetch(&s.pool, id).await?;
    tracing::info!("user '{}' ({role}) created by '{}'", n.name, actor.name);
    Ok((
        StatusCode::CREATED,
        Json(json!({"user": user, "token": token})),
    ))
}

pub async fn remove(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let target = users::fetch(&s.pool, id)
        .await
        .map_err(|_| ApiError::not_found("no such user"))?;
    if target.role == "owner" {
        return Err(ApiError::bad_request("owner cannot be deleted"));
    }
    // Revocation kills both channels: unpair certs in Wolf before dropping the account.
    for client_id in devices::client_ids_for_user(&s.pool, id).await? {
        s.wolf.unpair(&client_id).await?;
        tracing::info!(
            "device '{client_id}' unpaired (user '{}' deletion)",
            target.name
        );
    }
    users::remove(&s.pool, id).await?;
    tracing::info!("user '{}' deleted by '{}'", target.name, actor.name);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rotate_token(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let target = users::fetch(&s.pool, id)
        .await
        .map_err(|_| ApiError::not_found("no such user"))?;
    tokens::revoke_all(&s.pool, id).await?;
    let token = tokens::issue(&s.pool, id, "rotated").await?;
    tracing::info!("token rotated for '{}' by '{}'", target.name, actor.name);
    Ok(Json(json!({"token": token})))
}

#[derive(serde::Deserialize)]
pub struct Login {
    name: String,
    password: String,
}

/// Exchanges a name and password for a token.
///
/// Unauthenticated by necessity — it is how you obtain the credential
/// everything else requires. Tokens remain the auth mechanism for every other
/// request; this only gives a browser a way to get one, since it cannot hold a
/// token that was printed to a log once.
pub async fn login(
    State(s): State<AppState>,
    Json(body): Json<Login>,
) -> Result<Json<Value>, ApiError> {
    let Some(user_id) = crate::db::passwords::check(&s.pool, &body.name, &body.password).await
    else {
        // One message for every failure: a caller must not be able to
        // enumerate which accounts exist.
        tracing::warn!("failed login for '{}'", body.name);
        return Err(ApiError::unauthorized("invalid name or password"));
    };
    let user = crate::db::users::fetch(&s.pool, user_id).await?;
    let token = crate::db::tokens::issue(&s.pool, user_id, "login").await?;
    tracing::info!("'{}' signed in", user.name);
    Ok(Json(json!({ "token": token, "user": user })))
}

#[derive(serde::Deserialize)]
pub struct SetPassword {
    password: String,
}

/// Sets your own password. Needed by every account that predates passwords,
/// including the owner, whose token was printed to a log exactly once.
pub async fn set_own_password(
    State(s): State<AppState>,
    user: User,
    Json(body): Json<SetPassword>,
) -> Result<Json<Value>, ApiError> {
    crate::db::passwords::set(&s.pool, user.id, &body.password).await?;
    tracing::info!("'{}' set their password", user.name);
    Ok(Json(json!({ "ok": true })))
}
