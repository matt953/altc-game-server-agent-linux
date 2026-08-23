use sqlx::SqlitePool;

#[derive(Clone, Debug, sqlx::FromRow, serde::Serialize)]
pub struct Device {
    pub id: i64,
    pub user_id: i64,
    pub client_id: String,
    pub name: Option<String>,
    pub created_at: String,
}

// Re-approving an already-known cert rebinds it to the new owner.
pub async fn upsert(
    pool: &SqlitePool,
    user_id: i64,
    client_id: &str,
    name: &Option<String>,
) -> Result<Device, sqlx::Error> {
    sqlx::query_as::<_, Device>(
        "INSERT INTO devices (user_id, client_id, name) VALUES (?, ?, ?)
         ON CONFLICT(client_id) DO UPDATE SET user_id = excluded.user_id, name = excluded.name
         RETURNING id, user_id, client_id, name, created_at",
    )
    .bind(user_id)
    .bind(client_id)
    .bind(name)
    .fetch_one(pool)
    .await
}

pub async fn list_for_user(pool: &SqlitePool, user_id: i64) -> Result<Vec<Device>, sqlx::Error> {
    sqlx::query_as::<_, Device>(
        "SELECT id, user_id, client_id, name, created_at FROM devices WHERE user_id = ? ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

pub async fn client_ids_for_user(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT client_id FROM devices WHERE user_id = ?")
        .bind(user_id)
        .fetch_all(pool)
        .await
}
