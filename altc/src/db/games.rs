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
    pub video_producer_buffer_caps: String,
    pub render_node: String,
    /// The HostConfig blob every docker runner needs; identical across our
    /// games, so it is inherited rather than asked for on every add.
    pub runner_base_create_json: String,
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
             opus_gst_pipeline, start_audio_server, start_virtual_compositor,
             video_producer_buffer_caps, render_node, runner_base_create_json)
         VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            h264_gst_pipeline = excluded.h264_gst_pipeline,
            hevc_gst_pipeline = excluded.hevc_gst_pipeline,
            av1_gst_pipeline = excluded.av1_gst_pipeline,
            opus_gst_pipeline = excluded.opus_gst_pipeline,
            start_audio_server = excluded.start_audio_server,
            start_virtual_compositor = excluded.start_virtual_compositor,
            video_producer_buffer_caps = excluded.video_producer_buffer_caps,
            render_node = excluded.render_node,
            runner_base_create_json = excluded.runner_base_create_json",
    )
    .bind(&d.h264_gst_pipeline)
    .bind(&d.hevc_gst_pipeline)
    .bind(&d.av1_gst_pipeline)
    .bind(&d.opus_gst_pipeline)
    .bind(d.start_audio_server)
    .bind(d.start_virtual_compositor)
    .bind(&d.video_producer_buffer_caps)
    .bind(&d.render_node)
    .bind(&d.runner_base_create_json)
    .execute(pool)
    .await?;
    Ok(())
}

/// A fresh app id for a game we create ourselves.
///
/// Deliberately random, never derived from the title or icon: Wolf derives
/// `hash(icon_png_path + title)` when seeding from config, which means a
/// rename silently changes an app's identity, orphaning its shares and
/// invalidating every client's cached library. Ours must survive an edit.
///
/// Constrained to a positive signed 32-bit value because Moonlight clients
/// cannot carry anything wider.
pub async fn allocate_id(pool: &SqlitePool) -> Result<String, ApiError> {
    for _ in 0..64 {
        let raw: u32 = rand::random();
        let candidate = (1 + raw % (i32::MAX as u32 - 1)).to_string();
        let taken: Option<String> = sqlx::query_scalar("SELECT id FROM games WHERE id = ?")
            .bind(&candidate)
            .fetch_optional(pool)
            .await?;
        if taken.is_none() {
            return Ok(candidate);
        }
    }
    Err(ApiError::internal("could not allocate a free app id"))
}

/// Change only what a user can edit. The id is deliberately not a parameter:
/// identity must not move when a title or its art does.
pub async fn rename(pool: &SqlitePool, id: &str, title: &str, icon: &str) -> Result<(), ApiError> {
    let changed = sqlx::query("UPDATE games SET title = ?, icon_png_path = ? WHERE id = ?")
        .bind(title)
        .bind(icon)
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(ApiError::not_found("no such game"));
    }
    Ok(())
}

/// Removes the game and any per-user shares pointing at it. Shares are keyed
/// by app id with no foreign key to games, so without this they would linger
/// and silently re-attach if the id were ever reused.
pub async fn delete(pool: &SqlitePool, id: &str) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    let removed = sqlx::query("DELETE FROM games WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if removed == 0 {
        return Err(ApiError::not_found("no such game"));
    }
    sqlx::query("DELETE FROM app_shares WHERE app_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
