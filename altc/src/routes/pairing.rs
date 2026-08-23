use crate::auth::AdminUser;
use crate::db::{devices, users, users::User};
use crate::error::ApiError;
use crate::state::AppState;
use crate::wolf::{PairedClient, PendingPair};
use axum::{
    Json,
    extract::{Path, State},
};
use serde_json::{Value, json};

// Any authed user: self-pairing needs the pair_secret, which only the server knows.
pub async fn pending(
    State(s): State<AppState>,
    _user: User,
) -> Result<Json<Vec<PendingPair>>, ApiError> {
    Ok(Json(s.wolf.pending_pair_requests().await?))
}

#[derive(serde::Deserialize)]
pub struct PairApproval {
    pair_secret: String,
    pin: String,
    device_name: Option<String>,
    user_id: Option<i64>,
}

// Any authed user may approve; the approved device is bound to them.
// user_id (admin only) approves on behalf of another account.
pub async fn approve(
    State(s): State<AppState>,
    actor: User,
    Json(p): Json<PairApproval>,
) -> Result<Json<Value>, ApiError> {
    let owner_id = match p.user_id {
        Some(target) if target != actor.id => {
            if !actor.is_admin() {
                return Err(ApiError::forbidden(
                    "admin role required to pair for another user",
                ));
            }
            users::fetch(&s.pool, target)
                .await
                .map_err(|_| ApiError::not_found("no such user"))?
                .id
        }
        _ => actor.id,
    };
    let client_id = s.wolf.pair(&p.pair_secret, &p.pin).await?;
    let device = devices::upsert(&s.pool, owner_id, &client_id, &p.device_name).await?;
    tracing::info!(
        "device '{client_id}' paired to user id {owner_id} (approved by '{}')",
        actor.name
    );
    Ok(Json(json!({"success": true, "device": device})))
}

pub async fn my_devices(
    State(s): State<AppState>,
    user: User,
) -> Result<Json<Vec<devices::Device>>, ApiError> {
    Ok(Json(devices::list_for_user(&s.pool, user.id).await?))
}

#[derive(serde::Deserialize)]
pub struct ClaimRequest {
    user_id: i64,
    device_name: Option<String>,
}

// Attach an already-paired (stock Moonlight) device to an account.
pub async fn claim(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(client_id): Path<String>,
    Json(c): Json<ClaimRequest>,
) -> Result<Json<devices::Device>, ApiError> {
    users::fetch(&s.pool, c.user_id)
        .await
        .map_err(|_| ApiError::not_found("no such user"))?;
    let known = s.wolf.paired_clients().await?;
    if !known.iter().any(|k| k.client_id == client_id) {
        return Err(ApiError::not_found("no paired client with that id"));
    }
    let device = devices::upsert(&s.pool, c.user_id, &client_id, &c.device_name).await?;
    tracing::info!(
        "device '{client_id}' claimed for user id {} by '{}'",
        c.user_id,
        actor.name
    );
    Ok(Json(device))
}

pub async fn clients(
    State(s): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<PairedClient>>, ApiError> {
    Ok(Json(s.wolf.paired_clients().await?))
}

#[derive(serde::Deserialize)]
pub struct UnpairRequest {
    client_id: String,
}

pub async fn unpair(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Json(r): Json<UnpairRequest>,
) -> Result<Json<Value>, ApiError> {
    s.wolf.unpair(&r.client_id).await?;
    sqlx::query("DELETE FROM devices WHERE client_id = ?")
        .bind(&r.client_id)
        .execute(&s.pool)
        .await?;
    tracing::info!("client '{}' unpaired by '{}'", r.client_id, actor.name);
    Ok(Json(json!({"success": true})))
}
