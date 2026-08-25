pub mod devices;
pub mod games;
pub mod migrations;
pub mod passwords;
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

pub async fn run_schema(pool: &SqlitePool) {
    schema(pool).await
}

async fn schema(pool: &SqlitePool) {
    migrations::run(pool).await;
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
