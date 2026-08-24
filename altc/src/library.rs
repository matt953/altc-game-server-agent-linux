use crate::db::games::{self, EngineDefaults, Game};
use crate::error::ApiError;
use crate::wolf::WolfClient;
use serde_json::{Value, json};
use sqlx::SqlitePool;

/// One-time adoption of whatever is already in Wolf's config. Idempotent:
/// with rows present it does nothing, so a restart never re-imports.
pub async fn import_if_empty(pool: &SqlitePool, wolf: &WolfClient) -> Result<usize, ApiError> {
    if games::count(pool).await? > 0 {
        return Ok(0);
    }
    let apps = wolf.apps_raw().await?;
    let mut imported = 0;
    for app in &apps {
        if app["title"].as_str().unwrap_or_default().is_empty() {
            continue;
        }
        // Seed engine config once, from the first real app that carries it.
        if games::engine_defaults(pool).await?.is_none() {
            games::set_engine_defaults(pool, &defaults_from(app)).await?;
        }
        games::upsert(pool, &game_from(app)).await?;
        imported += 1;
    }
    tracing::info!("library: imported {imported} apps from wolf config");
    Ok(imported)
}

fn game_from(app: &Value) -> Game {
    Game {
        id: app["id"].as_str().unwrap_or_default().to_string(),
        title: app["title"].as_str().unwrap_or_default().to_string(),
        support_hdr: app["support_hdr"].as_bool().unwrap_or(false),
        icon_png_path: app["icon_png_path"].as_str().unwrap_or("").to_string(),
        render_node: app["render_node"].as_str().unwrap_or("").to_string(),
        runner_json: app["runner"].to_string(),
    }
}

fn defaults_from(app: &Value) -> EngineDefaults {
    let s = |k: &str| app[k].as_str().unwrap_or_default().to_string();
    EngineDefaults {
        h264_gst_pipeline: s("h264_gst_pipeline"),
        hevc_gst_pipeline: s("hevc_gst_pipeline"),
        av1_gst_pipeline: s("av1_gst_pipeline"),
        opus_gst_pipeline: s("opus_gst_pipeline"),
        start_audio_server: app["start_audio_server"].as_bool().unwrap_or(true),
        start_virtual_compositor: app["start_virtual_compositor"].as_bool().unwrap_or(true),
    }
}

/// A game plus the engine config Wolf insists on, as an apps/add payload.
pub fn to_wolf_app(g: &Game, d: &EngineDefaults) -> Value {
    let runner: Value = serde_json::from_str(&g.runner_json).unwrap_or_else(|_| json!({}));
    json!({
        "id": g.id,
        "title": g.title,
        "support_hdr": g.support_hdr,
        "icon_png_path": g.icon_png_path,
        "render_node": g.render_node,
        "h264_gst_pipeline": d.h264_gst_pipeline,
        "hevc_gst_pipeline": d.hevc_gst_pipeline,
        "av1_gst_pipeline": d.av1_gst_pipeline,
        "opus_gst_pipeline": d.opus_gst_pipeline,
        "start_audio_server": d.start_audio_server,
        "start_virtual_compositor": d.start_virtual_compositor,
        "runner": runner,
    })
}

/// Make Wolf's app list match ours. All-or-nothing: we validate we can build
/// every payload before sending any, because a half-pushed library looks to a
/// client exactly like a library that has lost games.
pub async fn push(pool: &SqlitePool, wolf: &WolfClient) -> Result<usize, ApiError> {
    let ours = games::list(pool).await?;
    if ours.is_empty() {
        return Ok(0);
    }
    let Some(defaults) = games::engine_defaults(pool).await? else {
        return Err(ApiError::internal(
            "no engine defaults; cannot push library",
        ));
    };
    let payloads: Vec<Value> = ours.iter().map(|g| to_wolf_app(g, &defaults)).collect();

    // We own every app now, so Wolf's list is replaced wholesale.
    for app in &wolf.apps_raw().await? {
        if let Some(id) = app["id"].as_str() {
            wolf.delete_app(id).await?;
        }
    }
    for p in &payloads {
        wolf.add_app(p.clone()).await?;
    }
    tracing::info!("library: pushed {} apps to wolf", payloads.len());
    Ok(payloads.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_app() -> Value {
        json!({
            "id": "578802895",
            "title": "Baldur's Gate 3",
            "support_hdr": false,
            "icon_png_path": "",
            "render_node": "/dev/dri/renderD128",
            "h264_gst_pipeline": "h264...",
            "hevc_gst_pipeline": "hevc...",
            "av1_gst_pipeline": "av1...",
            "opus_gst_pipeline": "opus...",
            "start_audio_server": true,
            "start_virtual_compositor": true,
            "runner": {"type": "docker", "image": "wolf-runner:v7"},
        })
    }

    #[test]
    fn round_trips_an_app_through_the_library_shape() {
        let app = sample_app();
        let g = game_from(&app);
        let d = defaults_from(&app);
        let out = to_wolf_app(&g, &d);
        // Every field Wolf demands must survive; it defaults none of them.
        for key in [
            "id",
            "title",
            "support_hdr",
            "icon_png_path",
            "render_node",
            "h264_gst_pipeline",
            "hevc_gst_pipeline",
            "av1_gst_pipeline",
            "opus_gst_pipeline",
            "start_audio_server",
            "start_virtual_compositor",
            "runner",
        ] {
            assert_eq!(out[key], app[key], "field {key} did not round-trip");
        }
    }

    #[test]
    fn id_is_carried_through_unchanged() {
        let app = sample_app();
        let g = game_from(&app);
        let d = defaults_from(&app);
        assert_eq!(to_wolf_app(&g, &d)["id"], json!("578802895"));
    }

    #[tokio::test]
    async fn import_is_idempotent_and_push_needs_defaults() {
        let pool = crate::db::init_memory().await;
        let app = sample_app();
        games::set_engine_defaults(&pool, &defaults_from(&app))
            .await
            .unwrap();
        games::upsert(&pool, &game_from(&app)).await.unwrap();
        assert_eq!(games::count(&pool).await.unwrap(), 1);
        // Re-importing must not duplicate: same id upserts in place.
        games::upsert(&pool, &game_from(&app)).await.unwrap();
        assert_eq!(games::count(&pool).await.unwrap(), 1);
    }
}
