use crate::auth::AdminUser;
use crate::db::{tokens, users, users::User};
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
