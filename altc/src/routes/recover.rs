use crate::db::{passwords, tokens, users};
use crate::error::ApiError;
use crate::state::AppState;
use axum::{Json, extract::State};
use serde_json::{Value, json};

/// Creating this file is how you prove you are at the machine.
///
/// No code to read out of a log — that is the pattern this pillar exists to
/// remove. Being able to write into the agent's state folder IS the proof, and
/// it cannot be done from the network. Deleted on success so recovery is not
/// left standing open.
pub const RECOVERY_FILE: &str = "recover";

pub fn recovery_path(state_dir: &std::path::Path) -> std::path::PathBuf {
    state_dir.join(RECOVERY_FILE)
}

/// Warns if recovery has been left armed. An unnoticed file here means anyone
/// who can reach the API can reset a password.
pub fn warn_if_armed(state_dir: &std::path::Path) {
    if recovery_path(state_dir).exists() {
        tracing::warn!(
            "recovery is armed: {} exists, so a password can be reset without signing in. \
             Delete it once you are done.",
            recovery_path(state_dir).display()
        );
    }
}

#[derive(serde::Deserialize)]
pub struct Recover {
    pub name: String,
    pub password: String,
}

/// Sets a new password for an account, authorised by the file rather than by a
/// credential — because the case this exists for is having no usable
/// credential at all.
///
/// Changes exactly one thing: that account's password. Accounts, devices,
/// shares and the library are untouched, which is the difference between
/// recovery and starting over.
pub async fn recover(
    State(s): State<AppState>,
    Json(body): Json<Recover>,
) -> Result<Json<Value>, ApiError> {
    let marker = recovery_path(&s.state_dir);
    if !marker.exists() {
        return Err(ApiError::forbidden(format!(
            "recovery is not enabled. Create {} on the server to prove you have access to it, \
             then try again.",
            marker.display()
        )));
    }

    let name = body.name.trim();
    let Some(user) = users::by_name(&s.pool, name).await? else {
        // The file already proves physical access, so naming the real problem
        // costs nothing and saves a confusing retry.
        return Err(ApiError::not_found(format!("no account called '{name}'")));
    };
    passwords::set(&s.pool, user.id, &body.password).await?;

    // Only after the password is safely changed: a failed attempt must leave
    // recovery armed, or one typo locks you out again.
    if let Err(e) = std::fs::remove_file(&marker) {
        tracing::error!(
            "password for '{name}' was reset but {} could not be removed: {e}. \
             Delete it manually — recovery is still armed.",
            marker.display()
        );
    }

    let token = tokens::issue(&s.pool, user.id, "recovery").await?;
    tracing::warn!("password for '{name}' was reset via the recovery file");
    Ok(Json(json!({ "token": token, "user": user })))
}
