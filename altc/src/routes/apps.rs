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
    /// Always something a client can actually fetch. Cached art is stored as a
    /// path inside the wolf container, which is meaningless to a client, so it
    /// is exposed as this API's own art endpoint instead.
    pub icon: Option<String>,
    // Added by the metadata pipeline; absent until a game has been identified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
    /// Absent means the client decides, which is the default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller_override: Option<String>,
    /// platinum/gold/silver/bronze/borked — what a client shows as a badge so
    /// a user knows whether a game actually works before launching it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protondb_tier: Option<String>,
    /// A short blurb; Steam's short_description. GOG has no equivalent — its
    /// "lead" is a duplicate of the full text, store banner included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genres: Option<String>,
}

/// A stored icon is either a URL the store gave us or a file we cached. Only
/// the first is any use to a client verbatim.
fn client_icon(g: &crate::db::games::Game) -> Option<String> {
    let stored = g.icon_png_path.trim();
    if stored.is_empty() {
        return None;
    }
    if stored.starts_with("http://") || stored.starts_with("https://") {
        return Some(stored.to_string());
    }
    Some(format!("/api/v1/apps/{}/art", g.id))
}

fn blank_to_none(s: &str) -> Option<String> {
    Some(s.to_string()).filter(|v| !v.is_empty())
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
            icon: client_icon(&g),
            description: blank_to_none(&g.description),
            release_date: blank_to_none(&g.release_date),
            store: blank_to_none(&g.store),
            controller_override: blank_to_none(&g.controller_override),
            protondb_tier: blank_to_none(&g.protondb_tier),
            tagline: blank_to_none(&g.tagline),
            developer: blank_to_none(&g.developer),
            genres: blank_to_none(&g.genres),
            id: g.id,
            title: g.title,
            support_hdr: g.support_hdr,
        })
        .collect();
    Ok(Json(apps))
}

pub async fn get_shares(
    State(s): State<AppState>,
    _admin: AdminUser,
    Path(app_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // Without this a deleted game answers "shared with everyone", which reads
    // to a UI as a real game that everyone can see.
    if !games::list(&s.pool).await?.iter().any(|g| g.id == app_id) {
        return Err(ApiError::not_found("no such app"));
    }
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
            icon: client_icon(&g),
            id: g.id,
            title: g.title,
            support_hdr: g.support_hdr,
        }
    }
}

pub async fn create(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Json(new): Json<crate::library::NewGame>,
) -> Result<(axum::http::StatusCode, Json<AdminApp>), ApiError> {
    let game = crate::library::create(&s.pool, &s.wolf, &s.library, new).await?;
    tracing::info!("app '{}' added by '{}'", game.title, actor.name);

    // Identify and decorate it immediately: "add a game" should mean the tile
    // is populated, not that someone must remember a second call. A metadata
    // failure must never undo an otherwise valid add, so it only warns.
    let decorated = match crate::metadata::fetch::HttpSource::new() {
        Ok(source) => {
            match crate::library::refresh_metadata(&s.pool, &s.wolf, &source, &s.art_dir, &game.id)
                .await
            {
                Ok(g) => g,
                Err(e) => {
                    tracing::warn!("metadata for '{}' unavailable: {}", game.title, e.1);
                    game
                }
            }
        }
        Err(e) => {
            tracing::warn!("no http client for metadata: {}", e.1);
            game
        }
    };
    Ok((axum::http::StatusCode::CREATED, Json(decorated.into())))
}

#[derive(serde::Deserialize)]
pub struct AppEdit {
    title: Option<String>,
    icon: Option<String>,
    /// XBOX / PS / NINTENDO / AUTO. Per-title, so one game can be forced to
    /// XInput without costing every other game on that client its DualSense.
    controller_override: Option<String>,
    /// Which GPU runs this game. Already per-app in Wolf; with two GPUs it is
    /// capacity as much as preference.
    render_node: Option<String>,
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

    if edit.controller_override.is_some() || edit.render_node.is_some() {
        let mut g = games::list(&s.pool)
            .await?
            .into_iter()
            .find(|g| g.id == app_id)
            .ok_or_else(|| ApiError::not_found("no such app"))?;
        if let Some(c) = edit.controller_override {
            let upper = c.trim().to_uppercase();
            if !matches!(upper.as_str(), "XBOX" | "PS" | "NINTENDO" | "AUTO" | "") {
                return Err(ApiError::bad_request(
                    "controller_override must be XBOX, PS, NINTENDO or AUTO",
                ));
            }
            // AUTO is stored as empty: "no opinion" and "explicitly automatic"
            // are the same thing, and one representation avoids ambiguity.
            g.controller_override = if upper == "AUTO" {
                String::new()
            } else {
                upper
            };
        }
        if let Some(r) = edit.render_node {
            g.render_node = r;
        }
        games::upsert(&s.pool, &g).await?;
    }
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

/// Identify a game from its install folder and fetch its metadata and art.
/// Nothing is accepted from the caller: the point is that a populated grid
/// proves the pipeline ran.
pub async fn refresh(
    State(s): State<AppState>,
    AdminUser(actor): AdminUser,
    Path(app_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let source = crate::metadata::fetch::HttpSource::new()?;
    let art_dir = s.art_dir.clone();
    let (pool, wolf) = (s.pool.clone(), s.wolf.clone());
    let game = crate::library::refresh_metadata(&pool, &wolf, &source, &art_dir, &app_id).await?;
    tracing::info!(
        "metadata for '{}' refreshed by '{}'",
        game.title,
        actor.name
    );
    Ok(Json(json!({
        "id": game.id,
        "title": game.title,
        "store": game.store,
        "store_id": game.store_id,
        "slug": game.slug,
        "release_date": game.release_date,
        "description": game.description,
        "protondb_tier": game.protondb_tier,
        "steam_appid": game.steam_appid,
        "appid_source": game.appid_source,
        "tagline": game.tagline,
        "developer": game.developer,
        "genres": game.genres,
        "has_art": !game.icon_png_path.is_empty(),
    })))
}

/// Serves the cached art. Clients render tiles from this; Wolf reads the file
/// directly for Moonlight's appasset, so stock clients get covers too.
pub async fn art(
    State(s): State<AppState>,
    _user: User,
    Path(app_id): Path<String>,
) -> Result<axum::response::Response, ApiError> {
    let game = games::list(&s.pool)
        .await?
        .into_iter()
        .find(|g| g.id == app_id)
        .ok_or_else(|| ApiError::not_found("no such app"))?;
    if game.icon_png_path.is_empty() {
        return Err(ApiError::not_found("no art for this game"));
    }
    let bytes = tokio::fs::read(&game.icon_png_path)
        .await
        .map_err(|_| ApiError::not_found("art file is missing"))?;
    let content_type = match std::path::Path::new(&game.icon_png_path)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    };
    Ok(axum::response::IntoResponse::into_response((
        [(axum::http::header::CONTENT_TYPE, content_type)],
        bytes,
    )))
}
