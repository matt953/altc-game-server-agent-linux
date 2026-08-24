use crate::auth::AdminUser;
use crate::error::ApiError;
use crate::state::AppState;
use axum::{
    Json,
    extract::{Query, State},
};
use serde_json::{Value, json};

/// The folders the admin may browse. A client shows these as the starting
/// points of the add-a-game form, the way Jellyfin lists library folders.
pub async fn roots(State(s): State<AppState>, _admin: AdminUser) -> Result<Json<Value>, ApiError> {
    let roots: Vec<Value> = s
        .library
        .roots()
        .iter()
        .map(|r| {
            let readable = r.is_dir();
            json!({
                "path": r.to_string_lossy(),
                // A root declared but not mounted is a deployment mistake the
                // admin needs to see, not a folder that silently has no games.
                "readable": readable,
            })
        })
        .collect();
    Ok(Json(json!({ "roots": roots })))
}

#[derive(serde::Deserialize)]
pub struct BrowseQuery {
    path: String,
}

pub async fn browse(
    State(s): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<BrowseQuery>,
) -> Result<Json<Value>, ApiError> {
    let entries = s.library.browse(&q.path)?;
    Ok(Json(json!({ "path": q.path, "entries": entries })))
}
