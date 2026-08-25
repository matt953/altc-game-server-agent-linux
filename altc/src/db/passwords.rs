use crate::error::ApiError;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use sqlx::SqlitePool;

/// Short enough not to be a chore on a TV remote, long enough to matter. The
/// server is reachable on a LAN or a VPN, not the open internet, so length
/// beats forced complexity rules that only push people to reuse passwords.
pub const MIN_LENGTH: usize = 8;

pub fn validate(password: &str) -> Result<(), ApiError> {
    if password.chars().count() < MIN_LENGTH {
        return Err(ApiError::bad_request(format!(
            "password must be at least {MIN_LENGTH} characters"
        )));
    }
    Ok(())
}

/// Argon2id, not a plain digest: a password is low-entropy and guessable, so
/// it needs a deliberately slow hash. Tokens elsewhere use SHA-256 and that is
/// correct for them — they are high-entropy random strings where the only
/// concern is not storing them in the clear.
pub fn hash(password: &str) -> Result<String, ApiError> {
    let mut raw = [0u8; 16];
    rand::fill(&mut raw);
    let salt =
        SaltString::encode_b64(&raw).map_err(|e| ApiError::internal(format!("salt: {e}")))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| ApiError::internal(format!("hash password: {e}")))
}

pub fn verify(password: &str, stored: &str) -> bool {
    match PasswordHash::new(stored) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

pub async fn set(pool: &SqlitePool, user_id: i64, password: &str) -> Result<(), ApiError> {
    validate(password)?;
    let hashed = hash(password)?;
    let changed = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(&hashed)
        .bind(user_id)
        .execute(pool)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(ApiError::not_found("no such user"));
    }
    Ok(())
}

/// Checks a name and password. Returns the user id on success.
///
/// A user with no password set cannot log in this way at all — which is every
/// account that existed before passwords did, including the owner. They stay
/// valid via their token until someone sets one.
pub async fn check(pool: &SqlitePool, name: &str, password: &str) -> Option<i64> {
    let row: Option<(i64, Option<String>)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE name = ?")
            .bind(name)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

    match row {
        Some((id, Some(stored))) if verify(password, &stored) => Some(id),
        // Deliberately identical for "no such user", "no password set" and
        // "wrong password": the caller must not learn which accounts exist.
        _ => None,
    }
}

pub async fn has_password(pool: &SqlitePool, user_id: i64) -> bool {
    sqlx::query_scalar::<_, Option<String>>("SELECT password_hash FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_verifies_against_its_own_hash() {
        let h = hash("correct horse battery").unwrap();
        assert!(verify("correct horse battery", &h));
        assert!(!verify("wrong horse battery", &h));
    }

    #[test]
    fn the_same_password_hashes_differently_every_time() {
        // Salted: two users with the same password must not be visibly equal
        // in the database.
        let a = hash("same password").unwrap();
        let b = hash("same password").unwrap();
        assert_ne!(a, b);
        assert!(verify("same password", &a) && verify("same password", &b));
    }

    #[test]
    fn the_stored_form_is_not_the_password() {
        let h = hash("hunter2hunter2").unwrap();
        assert!(!h.contains("hunter2"));
        assert!(h.starts_with("$argon2"), "got {h}");
    }

    #[test]
    fn garbage_in_the_column_fails_closed() {
        // A corrupted or hand-edited row must reject, never accept.
        assert!(!verify("anything", ""));
        assert!(!verify("anything", "not-a-hash"));
        assert!(!verify("anything", "$argon2id$broken"));
    }

    #[test]
    fn too_short_is_refused() {
        assert!(validate("short").is_err());
        assert!(validate("longenough").is_ok());
        // Counted in characters, not bytes, so non-ASCII is not penalised.
        assert!(validate("übergänge").is_ok());
    }

    #[tokio::test]
    async fn login_fails_the_same_way_for_every_reason() {
        let pool = crate::db::init_memory().await;
        let id = crate::db::users::insert(&pool, "dave", "member")
            .await
            .unwrap();
        set(&pool, id, "dave's password").await.unwrap();

        assert_eq!(check(&pool, "dave", "dave's password").await, Some(id));
        assert_eq!(check(&pool, "dave", "wrong").await, None);
        assert_eq!(check(&pool, "nobody", "dave's password").await, None);

        // An account with no password cannot be logged into, but still exists.
        let tokenly = crate::db::users::insert(&pool, "owner", "owner")
            .await
            .unwrap();
        assert_eq!(check(&pool, "owner", "").await, None);
        assert_eq!(check(&pool, "owner", "anything").await, None);
        assert!(!has_password(&pool, tokenly).await);
        assert!(has_password(&pool, id).await);
    }
}
