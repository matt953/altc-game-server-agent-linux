use crate::db::users::User;
use crate::error::ApiError;
use crate::state::AppState;
use axum::{Json, extract::State};

#[derive(serde::Serialize)]
pub struct ClientApp {
    pub id: String,
    pub title: String,
    pub support_hdr: bool,
    pub icon: Option<String>,
}

pub async fn list(
    State(s): State<AppState>,
    _user: User,
) -> Result<Json<Vec<ClientApp>>, ApiError> {
    let apps = s
        .wolf
        .apps()
        .await?
        .into_iter()
        .map(|a| ClientApp {
            id: a.id,
            title: a.title,
            support_hdr: a.support_hdr,
            icon: a.icon_png_path.filter(|p| !p.is_empty()),
        })
        .collect();
    Ok(Json(apps))
}
