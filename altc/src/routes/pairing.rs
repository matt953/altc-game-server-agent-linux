use crate::auth::AdminUser;
use crate::error::ApiError;
use crate::state::AppState;
use crate::wolf::{PairedClient, PendingPair};
use axum::{Json, extract::State};
use serde_json::{Value, json};

pub async fn pending(
    State(s): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<PendingPair>>, ApiError> {
    Ok(Json(s.wolf.pending_pair_requests().await?))
}

#[derive(serde::Deserialize)]
pub struct PairApproval {
    pair_secret: String,
    pin: String,
}

pub async fn approve(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Json(p): Json<PairApproval>,
) -> Result<Json<Value>, ApiError> {
    s.wolf.pair(&p.pair_secret, &p.pin).await?;
    tracing::info!("pair request approved by '{}'", actor.name);
    Ok(Json(json!({"success": true})))
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
    tracing::info!("client '{}' unpaired by '{}'", r.client_id, actor.name);
    Ok(Json(json!({"success": true})))
}
