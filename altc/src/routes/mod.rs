mod me;
mod pairing;
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
        .route("/api/v1/pair/pending", get(pairing::pending))
        .route("/api/v1/pair", post(pairing::approve))
        .route("/api/v1/clients", get(pairing::clients))
        .route("/api/v1/unpair", post(pairing::unpair))
        .with_state(state)
}

async fn healthz() -> Json<Value> {
    Json(json!({"status": "ok", "service": "altc-api", "version": env!("CARGO_PKG_VERSION")}))
}
