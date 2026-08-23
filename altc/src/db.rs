use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use std::path::Path;

pub async fn init(state_dir: &Path) -> SqlitePool {
    let opts = SqliteConnectOptions::new()
        .filename(state_dir.join("altc.db"))
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.expect("open altc.db");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            role TEXT NOT NULL CHECK (role IN ('owner','admin','member')),
            locale TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS tokens (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            token_hash TEXT NOT NULL UNIQUE,
            label TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_used_at TEXT
        );",
    )
    .execute(&pool)
    .await
    .expect("create schema");

    pool
}

// First boot: create the owner and print their token exactly once.
pub async fn bootstrap_owner(pool: &SqlitePool) {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .expect("count users");
    if count == 0 {
        let token = crate::auth::new_token();
        sqlx::query("INSERT INTO users (name, role) VALUES ('owner', 'owner')")
            .execute(pool)
            .await
            .expect("insert owner");
        sqlx::query("INSERT INTO tokens (user_id, token_hash, label) VALUES (1, ?, 'bootstrap')")
            .bind(crate::auth::hash_token(&token))
            .execute(pool)
            .await
            .expect("insert owner token");
        tracing::warn!("FIRST BOOT — OWNER TOKEN (shown once, store it now): {token}");
    }
}
