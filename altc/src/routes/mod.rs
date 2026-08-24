mod apps;
mod events;
mod me;
mod pairing;
mod storage;
mod users;

use crate::state::AppState;
use axum::{
    Json, Router,
    routing::{delete, get, patch, post},
};
use serde_json::{Value, json};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/me", get(me::me))
        .route("/api/v1/me/settings", patch(me::update_settings))
        .route("/api/v1/users", get(users::list).post(users::create))
        .route("/api/v1/users/{id}", delete(users::remove))
        .route("/api/v1/users/{id}/tokens", post(users::rotate_token))
        .route("/api/v1/events", get(events::stream))
        .route("/api/v1/storage/roots", get(storage::roots))
        .route("/api/v1/storage/browse", get(storage::browse))
        .route("/api/v1/apps", get(apps::list).post(apps::create))
        .route(
            "/api/v1/apps/{app_id}",
            axum::routing::patch(apps::update).delete(apps::delete),
        )
        .route(
            "/api/v1/apps/{app_id}/shares",
            get(apps::get_shares).put(apps::set_shares),
        )
        .route("/api/v1/me/devices", get(pairing::my_devices))
        .route("/api/v1/pair/pending", get(pairing::pending))
        .route("/api/v1/pair", post(pairing::approve))
        .route("/api/v1/devices/{client_id}/claim", post(pairing::claim))
        .route("/api/v1/clients", get(pairing::clients))
        .route("/api/v1/unpair", post(pairing::unpair))
        .with_state(state)
}

async fn healthz() -> Json<Value> {
    Json(json!({"status": "ok", "service": "altc-api", "version": env!("CARGO_PKG_VERSION")}))
}
