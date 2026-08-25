use sqlx::SqlitePool;

#[derive(Clone, Debug, sqlx::FromRow, serde::Serialize)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub role: String,
    pub locale: Option<String>,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role == "owner" || self.role == "admin"
    }
}

pub async fn fetch(pool: &SqlitePool, id: i64) -> Result<User, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT id, name, role, locale FROM users WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<User>, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT id, name, role, locale FROM users ORDER BY id")
        .fetch_all(pool)
        .await
}

pub async fn insert(pool: &SqlitePool, name: &str, role: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("INSERT INTO users (name, role) VALUES (?, ?) RETURNING id")
        .bind(name)
        .bind(role)
        .fetch_one(pool)
        .await
}

pub async fn remove(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn set_locale(
    pool: &SqlitePool,
    id: i64,
    locale: &Option<String>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET locale = ? WHERE id = ?")
        .bind(locale)
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Looks a user up by the name they sign in with.
pub async fn by_name(pool: &SqlitePool, name: &str) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await
}
