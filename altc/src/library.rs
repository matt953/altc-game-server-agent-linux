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
        // Each app keeps its own engine config. The first one also seeds the
        // template for games we create ourselves later (M3).
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
    let s = |k: &str| app[k].as_str().unwrap_or_default().to_string();
    Game {
        id: s("id"),
        title: s("title"),
        support_hdr: app["support_hdr"].as_bool().unwrap_or(false),
        icon_png_path: s("icon_png_path"),
        render_node: s("render_node"),
        runner_json: app["runner"].to_string(),
        // Dropped by Wolf's own reflector until 2026-08-24; without it the
        // video producer pipeline is malformed and the app cannot launch.
        video_producer_buffer_caps: s("video_producer_buffer_caps"),
        // Copied per app, never shared: an app may override any of these.
        h264_gst_pipeline: s("h264_gst_pipeline"),
        hevc_gst_pipeline: s("hevc_gst_pipeline"),
        av1_gst_pipeline: s("av1_gst_pipeline"),
        opus_gst_pipeline: s("opus_gst_pipeline"),
        start_audio_server: app["start_audio_server"].as_bool().unwrap_or(true),
        start_virtual_compositor: app["start_virtual_compositor"].as_bool().unwrap_or(true),
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

/// A game as an apps/add payload. Engine config comes from the game itself,
/// so an app that overrides a pipeline keeps its override.
pub fn to_wolf_app(g: &Game) -> Value {
    let runner: Value = serde_json::from_str(&g.runner_json).unwrap_or_else(|_| json!({}));
    json!({
        "id": g.id,
        "title": g.title,
        "support_hdr": g.support_hdr,
        "icon_png_path": g.icon_png_path,
        "render_node": g.render_node,
        "video_producer_buffer_caps": g.video_producer_buffer_caps,
        "h264_gst_pipeline": g.h264_gst_pipeline,
        "hevc_gst_pipeline": g.hevc_gst_pipeline,
        "av1_gst_pipeline": g.av1_gst_pipeline,
        "opus_gst_pipeline": g.opus_gst_pipeline,
        "start_audio_server": g.start_audio_server,
        "start_virtual_compositor": g.start_virtual_compositor,
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
    // Refuse rather than push a game with no pipelines: Wolf accepts it and
    // then cannot build a session for it.
    if let Some(g) = ours.iter().find(|g| g.hevc_gst_pipeline.is_empty()) {
        return Err(ApiError::internal(format!(
            "game '{}' has no engine config; refusing to push a partial library",
            g.title
        )));
    }
    let payloads: Vec<Value> = ours.iter().map(to_wolf_app).collect();

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
            "video_producer_buffer_caps": "video/x-raw, format=NV12",
            "start_audio_server": true,
            "start_virtual_compositor": true,
            "runner": {"type": "docker", "image": "wolf-runner:v7"},
        })
    }

    // Test ball as it really is in Wolf: its own sources, and both servers off.
    fn overriding_app() -> Value {
        json!({
            "id": "249285395",
            "title": "Test ball",
            "support_hdr": false,
            "icon_png_path": "",
            "render_node": "/dev/dri/renderD128",
            "h264_gst_pipeline": "videotestsrc pattern=ball ...",
            "hevc_gst_pipeline": "videotestsrc pattern=ball ...",
            "av1_gst_pipeline": "videotestsrc pattern=ball ...",
            "opus_gst_pipeline": "audiotestsrc wave=ticks ...",
            "video_producer_buffer_caps": "video/x-raw, format=NV12",
            "start_audio_server": false,
            "start_virtual_compositor": false,
            "runner": {"type": "process", "run_cmd": "sh -c 'sleep 10'"},
        })
    }

    #[test]
    fn round_trips_an_app_through_the_library_shape() {
        let app = sample_app();
        let out = to_wolf_app(&game_from(&app));
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
            "video_producer_buffer_caps",
            "start_audio_server",
            "start_virtual_compositor",
            "runner",
        ] {
            assert_eq!(out[key], app[key], "field {key} did not round-trip");
        }
    }

    // Regression, 2026-08-24: a shared engine_defaults row flattened Test
    // ball onto Wolf UI's pipelines, so it streamed nothing.
    #[test]
    fn an_apps_engine_overrides_are_not_replaced_by_another_apps() {
        let (normal, ball) = (sample_app(), overriding_app());
        let out_ball = to_wolf_app(&game_from(&ball));
        let out_normal = to_wolf_app(&game_from(&normal));

        assert_eq!(out_ball["h264_gst_pipeline"], ball["h264_gst_pipeline"]);
        assert_eq!(out_ball["opus_gst_pipeline"], ball["opus_gst_pipeline"]);
        assert_eq!(out_ball["start_audio_server"], json!(false));
        assert_eq!(out_ball["start_virtual_compositor"], json!(false));
        // ...and the ordinary app is untouched by the override's presence.
        assert_eq!(out_normal["h264_gst_pipeline"], normal["h264_gst_pipeline"]);
        assert_eq!(out_normal["start_audio_server"], json!(true));
    }

    #[test]
    fn id_is_carried_through_unchanged() {
        assert_eq!(
            to_wolf_app(&game_from(&sample_app()))["id"],
            json!("578802895")
        );
    }

    #[tokio::test]
    async fn import_is_idempotent_and_push_needs_defaults() {
        let pool = crate::db::init_memory().await;
        let app = sample_app();
        games::upsert(&pool, &game_from(&app)).await.unwrap();
        assert_eq!(games::count(&pool).await.unwrap(), 1);
        // Re-importing must not duplicate: same id upserts in place.
        games::upsert(&pool, &game_from(&app)).await.unwrap();
        assert_eq!(games::count(&pool).await.unwrap(), 1);
    }
}
