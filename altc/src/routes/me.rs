use crate::db::users::{self, User};
use crate::error::ApiError;
use crate::state::AppState;
use axum::{Json, extract::State};

pub async fn me(user: User) -> Json<User> {
    Json(user)
}

#[derive(serde::Deserialize)]
pub struct SettingsPatch {
    locale: Option<String>,
}

pub async fn update_settings(
    State(s): State<AppState>,
    user: User,
    Json(p): Json<SettingsPatch>,
) -> Result<Json<User>, ApiError> {
    users::set_locale(&s.pool, user.id, &p.locale).await?;
    Ok(Json(users::fetch(&s.pool, user.id).await?))
}
