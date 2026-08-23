use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

pub fn new_token() -> String {
    let b: [u8; 32] = rand::random();
    format!("altc_{}", hex::encode(b))
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[derive(Clone, Debug, sqlx::FromRow, serde::Serialize)]
pub struct AuthedUser {
    pub id: i64,
    pub name: String,
    pub role: String,
    pub locale: Option<String>,
}

impl AuthedUser {
    pub fn is_admin(&self) -> bool {
        self.role == "owner" || self.role == "admin"
    }
}

impl FromRequestParts<crate::AppState> for AuthedUser {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &crate::AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or((StatusCode::UNAUTHORIZED, "missing bearer token"))?;
        let user = lookup(&state.pool, token)
            .await
            .ok_or((StatusCode::UNAUTHORIZED, "invalid token"))?;
        Ok(user)
    }
}

pub struct AdminUser(pub AuthedUser);

impl FromRequestParts<crate::AppState> for AdminUser {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &crate::AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthedUser::from_request_parts(parts, state).await?;
        if !user.is_admin() {
            return Err((StatusCode::FORBIDDEN, "admin role required"));
        }
        Ok(AdminUser(user))
    }
}

async fn lookup(pool: &SqlitePool, token: &str) -> Option<AuthedUser> {
    let hash = hash_token(token);
    let user = sqlx::query_as::<_, AuthedUser>(
        "SELECT u.id, u.name, u.role, u.locale FROM users u
         JOIN tokens t ON t.user_id = u.id WHERE t.token_hash = ?",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await
    .ok()??;
    let _ = sqlx::query("UPDATE tokens SET last_used_at = datetime('now') WHERE token_hash = ?")
        .bind(&hash)
        .execute(pool)
        .await;
    Some(user)
}
