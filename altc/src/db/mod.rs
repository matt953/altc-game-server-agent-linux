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

/// Reports whether anyone has claimed this server yet.
///
/// No longer creates an owner or prints a token. That old behaviour assumed
/// shell access to read the log, left the credential sitting in logs forever,
/// and had no recovery path. The owner is now created by POST /api/v1/setup,
/// which closes permanently once used.
pub async fn report_claim_state(pool: &SqlitePool) {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .expect("count users");
    if count == 0 {
        tracing::warn!(
            "This server has not been claimed yet. Open it in a browser to create the owner \
             account; the claim window closes for good once someone does."
        );
    }
}
