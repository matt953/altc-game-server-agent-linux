use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

// app_id -> users it is restricted to. Absent app_id = visible to everyone.
pub async fn restrictions(pool: &SqlitePool) -> Result<HashMap<String, HashSet<i64>>, sqlx::Error> {
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT app_id, user_id FROM app_shares")
        .fetch_all(pool)
        .await?;
    let mut map: HashMap<String, HashSet<i64>> = HashMap::new();
    for (app_id, user_id) in rows {
        map.entry(app_id).or_default().insert(user_id);
    }
    Ok(map)
}

pub async fn for_app(pool: &SqlitePool, app_id: &str) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT user_id FROM app_shares WHERE app_id = ? ORDER BY user_id")
        .bind(app_id)
        .fetch_all(pool)
        .await
}

// Empty user_ids clears the restriction (back to everyone).
pub async fn set_for_app(
    pool: &SqlitePool,
    app_id: &str,
    user_ids: &[i64],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM app_shares WHERE app_id = ?")
        .bind(app_id)
        .execute(&mut *tx)
        .await?;
    for id in user_ids {
        sqlx::query("INSERT INTO app_shares (app_id, user_id) VALUES (?, ?)")
            .bind(app_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}
