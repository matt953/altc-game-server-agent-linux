use crate::db::{tokens, users::User};
use crate::error::ApiError;
use crate::state::AppState;
use axum::{extract::FromRequestParts, http::request::Parts};
use sha2::{Digest, Sha256};

pub fn new_token() -> String {
    let b: [u8; 32] = rand::random();
    format!("altc_{}", hex::encode(b))
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

impl FromRequestParts<AppState> for User {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
        tokens::lookup_user(&state.pool, token)
            .await
            .ok_or_else(|| ApiError::unauthorized("invalid token"))
    }
}

pub struct AdminUser(pub User);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let user = User::from_request_parts(parts, state).await?;
        if !user.is_admin() {
            return Err(ApiError::forbidden("admin role required"));
        }
        Ok(AdminUser(user))
    }
}
