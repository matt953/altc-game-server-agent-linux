use crate::auth::AdminUser;
use crate::db::{games, shares, users, users::User};
use crate::error::ApiError;
use crate::state::AppState;
use axum::{
    Json,
    extract::{Path, State},
};
use serde_json::{Value, json};

#[derive(serde::Serialize)]
pub struct ClientApp {
    pub id: String,
    pub title: String,
    pub support_hdr: bool,
    pub icon: Option<String>,
}

// Served from our own library, not proxied from Wolf: SQLite is the source of
// truth, and Wolf holds nothing when it has not been pushed to yet.
pub async fn list(State(s): State<AppState>, user: User) -> Result<Json<Vec<ClientApp>>, ApiError> {
    let restricted = shares::restrictions(&s.pool).await?;
    let apps = games::list(&s.pool)
        .await?
        .into_iter()
        .filter(|g| match restricted.get(&g.id) {
            Some(allowed) => user.is_admin() || allowed.contains(&user.id),
            None => true,
        })
        .map(|g| ClientApp {
            id: g.id,
            title: g.title,
            support_hdr: g.support_hdr,
            icon: Some(g.icon_png_path).filter(|p| !p.is_empty()),
        })
        .collect();
    Ok(Json(apps))
}

pub async fn get_shares(
    State(s): State<AppState>,
    _admin: AdminUser,
    Path(app_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let user_ids = shares::for_app(&s.pool, &app_id).await?;
    Ok(Json(json!({
        "app_id": app_id,
        "everyone": user_ids.is_empty(),
        "user_ids": user_ids,
    })))
}

#[derive(serde::Deserialize)]
pub struct SharesUpdate {
    user_ids: Vec<i64>,
}

pub async fn set_shares(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(app_id): Path<String>,
    Json(u): Json<SharesUpdate>,
) -> Result<Json<Value>, ApiError> {
    if !games::list(&s.pool).await?.iter().any(|g| g.id == app_id) {
        return Err(ApiError::not_found("no such app"));
    }
    for id in &u.user_ids {
        users::fetch(&s.pool, *id)
            .await
            .map_err(|_| ApiError::bad_request(format!("no such user: {id}")))?;
    }
    shares::set_for_app(&s.pool, &app_id, &u.user_ids).await?;
    tracing::info!(
        "app '{app_id}' shares set to {:?} by '{}'",
        u.user_ids,
        actor.name
    );
    Ok(Json(json!({
        "app_id": app_id,
        "everyone": u.user_ids.is_empty(),
        "user_ids": u.user_ids,
    })))
}

#[derive(serde::Serialize)]
pub struct AdminApp {
    pub id: String,
    pub title: String,
    pub support_hdr: bool,
    pub icon: Option<String>,
}

impl From<crate::db::games::Game> for AdminApp {
    fn from(g: crate::db::games::Game) -> Self {
        Self {
            id: g.id,
            title: g.title,
            support_hdr: g.support_hdr,
            icon: Some(g.icon_png_path).filter(|p| !p.is_empty()),
        }
    }
}

pub async fn create(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Json(new): Json<crate::library::NewGame>,
) -> Result<(axum::http::StatusCode, Json<AdminApp>), ApiError> {
    let game = crate::library::create(&s.pool, &s.wolf, new).await?;
    tracing::info!("app '{}' added by '{}'", game.title, actor.name);
    Ok((axum::http::StatusCode::CREATED, Json(game.into())))
}

#[derive(serde::Deserialize)]
pub struct AppEdit {
    title: Option<String>,
    icon: Option<String>,
}

/// Title and art only. The id is never editable: shares and every client's
/// cached library are keyed by it.
pub async fn update(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(app_id): Path<String>,
    Json(edit): Json<AppEdit>,
) -> Result<Json<AdminApp>, ApiError> {
    let current = games::list(&s.pool)
        .await?
        .into_iter()
        .find(|g| g.id == app_id)
        .ok_or_else(|| ApiError::not_found("no such app"))?;
    let title = edit.title.unwrap_or_else(|| current.title.clone());
    if title.trim().is_empty() {
        return Err(ApiError::bad_request("title cannot be empty"));
    }
    let icon = edit.icon.unwrap_or_else(|| current.icon_png_path.clone());
    games::rename(&s.pool, &app_id, title.trim(), &icon).await?;
    crate::library::push(&s.pool, &s.wolf).await?;
    tracing::info!("app '{}' edited by '{}'", title.trim(), actor.name);

    let updated = games::list(&s.pool)
        .await?
        .into_iter()
        .find(|g| g.id == app_id)
        .ok_or_else(|| ApiError::not_found("no such app"))?;
    Ok(Json(updated.into()))
}

pub async fn delete(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(app_id): Path<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    crate::library::remove(&s.pool, &s.wolf, &app_id).await?;
    tracing::info!("app {app_id} deleted by '{}'", actor.name);
    Ok(axum::http::StatusCode::NO_CONTENT)
}
