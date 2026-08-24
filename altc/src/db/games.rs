use crate::error::ApiError;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

// Engine config is per app in Wolf's model, not global: Test ball overrides
// all four pipelines with videotestsrc/audiotestsrc and turns the compositor
// and audio server off. Storing one shared copy flattened that (2026-08-24),
// so every app carries its own. engine_defaults is only a template for games
// we create ourselves.
#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub support_hdr: bool,
    pub icon_png_path: String,
    pub render_node: String,
    pub runner_json: String,
    pub video_producer_buffer_caps: String,
    pub h264_gst_pipeline: String,
    pub hevc_gst_pipeline: String,
    pub av1_gst_pipeline: String,
    pub opus_gst_pipeline: String,
    pub start_audio_server: bool,
    pub start_virtual_compositor: bool,
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<Game>, ApiError> {
    Ok(
        sqlx::query_as::<_, Game>("SELECT * FROM games ORDER BY title")
            .fetch_all(pool)
            .await?,
    )
}

pub async fn count(pool: &SqlitePool) -> Result<i64, ApiError> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM games")
        .fetch_one(pool)
        .await?)
}

pub async fn upsert(pool: &SqlitePool, g: &Game) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO games (id, title, support_hdr, icon_png_path, render_node, runner_json,
                            video_producer_buffer_caps, h264_gst_pipeline, hevc_gst_pipeline, av1_gst_pipeline,
                            opus_gst_pipeline, start_audio_server, start_virtual_compositor)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            title = excluded.title,
            support_hdr = excluded.support_hdr,
            icon_png_path = excluded.icon_png_path,
            render_node = excluded.render_node,
            runner_json = excluded.runner_json,
            video_producer_buffer_caps = excluded.video_producer_buffer_caps,
            h264_gst_pipeline = excluded.h264_gst_pipeline,
            hevc_gst_pipeline = excluded.hevc_gst_pipeline,
            av1_gst_pipeline = excluded.av1_gst_pipeline,
            opus_gst_pipeline = excluded.opus_gst_pipeline,
            start_audio_server = excluded.start_audio_server,
            start_virtual_compositor = excluded.start_virtual_compositor",
    )
    .bind(&g.id)
    .bind(&g.title)
    .bind(g.support_hdr)
    .bind(&g.icon_png_path)
    .bind(&g.render_node)
    .bind(&g.runner_json)
    .bind(&g.video_producer_buffer_caps)
    .bind(&g.h264_gst_pipeline)
    .bind(&g.hevc_gst_pipeline)
    .bind(&g.av1_gst_pipeline)
    .bind(&g.opus_gst_pipeline)
    .bind(g.start_audio_server)
    .bind(g.start_virtual_compositor)
    .execute(pool)
    .await?;
    Ok(())
}

/// Template used when WE create a game (M3). Never imposed on an imported one.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct EngineDefaults {
    pub h264_gst_pipeline: String,
    pub hevc_gst_pipeline: String,
    pub av1_gst_pipeline: String,
    pub opus_gst_pipeline: String,
    pub start_audio_server: bool,
    pub start_virtual_compositor: bool,
}

pub async fn engine_defaults(pool: &SqlitePool) -> Result<Option<EngineDefaults>, ApiError> {
    Ok(
        sqlx::query_as::<_, EngineDefaults>("SELECT * FROM engine_defaults WHERE id = 1")
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn set_engine_defaults(pool: &SqlitePool, d: &EngineDefaults) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO engine_defaults
            (id, h264_gst_pipeline, hevc_gst_pipeline, av1_gst_pipeline,
             opus_gst_pipeline, start_audio_server, start_virtual_compositor)
         VALUES (1, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            h264_gst_pipeline = excluded.h264_gst_pipeline,
            hevc_gst_pipeline = excluded.hevc_gst_pipeline,
            av1_gst_pipeline = excluded.av1_gst_pipeline,
            opus_gst_pipeline = excluded.opus_gst_pipeline,
            start_audio_server = excluded.start_audio_server,
            start_virtual_compositor = excluded.start_virtual_compositor",
    )
    .bind(&d.h264_gst_pipeline)
    .bind(&d.hevc_gst_pipeline)
    .bind(&d.av1_gst_pipeline)
    .bind(&d.opus_gst_pipeline)
    .bind(d.start_audio_server)
    .bind(d.start_virtual_compositor)
    .execute(pool)
    .await?;
    Ok(())
}
