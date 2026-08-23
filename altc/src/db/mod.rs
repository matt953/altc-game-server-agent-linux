pub mod devices;
pub mod shares;
pub mod tokens;
pub mod users;

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::path::Path;

pub async fn init(state_dir: &Path) -> SqlitePool {
    let opts = SqliteConnectOptions::new()
        .filename(state_dir.join("altc.db"))
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await.expect("open altc.db");
    schema(&pool).await;
    pool
}

// Single connection: each :memory: connection is its own database.
pub async fn init_memory() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open in-memory db");
    schema(&pool).await;
    pool
}

async fn schema(pool: &SqlitePool) {
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
        );
        CREATE TABLE IF NOT EXISTS devices (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            client_id TEXT NOT NULL UNIQUE,
            name TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        -- No rows for an app_id = shared with everyone (default).
        CREATE TABLE IF NOT EXISTS app_shares (
            app_id TEXT NOT NULL,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            PRIMARY KEY (app_id, user_id)
        );",
    )
    .execute(pool)
    .await
    .expect("create schema");
}

// First boot: create the owner and print their token exactly once.
pub async fn bootstrap_owner(pool: &SqlitePool) {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .expect("count users");
    if count == 0 {
        let id = users::insert(pool, "owner", "owner")
            .await
            .expect("insert owner");
        let token = tokens::issue(pool, id, "bootstrap")
            .await
            .expect("insert owner token");
        tracing::warn!("FIRST BOOT — OWNER TOKEN (shown once, store it now): {token}");
    }
}
