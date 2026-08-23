use crate::auth;
use crate::db::users::User;
use sqlx::SqlitePool;

pub async fn issue(pool: &SqlitePool, user_id: i64, label: &str) -> Result<String, sqlx::Error> {
    let token = auth::new_token();
    sqlx::query("INSERT INTO tokens (user_id, token_hash, label) VALUES (?, ?, ?)")
        .bind(user_id)
        .bind(auth::hash_token(&token))
        .bind(label)
        .execute(pool)
        .await?;
    Ok(token)
}

pub async fn revoke_all(pool: &SqlitePool, user_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM tokens WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn lookup_user(pool: &SqlitePool, token: &str) -> Option<User> {
    let hash = auth::hash_token(token);
    let user = sqlx::query_as::<_, User>(
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
