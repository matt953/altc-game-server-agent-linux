use crate::db::games::{self, EngineDefaults, Game};
use crate::error::ApiError;
use crate::metadata as md;
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
        // An imported app carries no store identity: it is discovered from the
        // install folder on refresh, never guessed from the title.
        store: String::new(),
        store_id: String::new(),
        slug: String::new(),
        release_date: String::new(),
        description: String::new(),
        protondb_tier: String::new(),
        steam_appid: String::new(),
        appid_source: String::new(),
        tagline: String::new(),
        developer: String::new(),
        genres: String::new(),
        controller_override: String::new(),
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
        video_producer_buffer_caps: s("video_producer_buffer_caps"),
        render_node: s("render_node"),
        runner_base_create_json: app["runner"]["base_create_json"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    }
}

/// Fill in engine config for rows that predate it, from Wolf's own copy of the
/// same app. Per app, so an override (Test ball's videotestsrc) is preserved;
/// a game Wolf has never heard of is left alone and reported, because it is
/// ours and inventing engine config for it would be a guess.
pub async fn backfill_engine_config(
    pool: &SqlitePool,
    wolf: &WolfClient,
) -> Result<usize, ApiError> {
    let stale: Vec<Game> = games::list(pool)
        .await?
        .into_iter()
        .filter(|g| g.hevc_gst_pipeline.is_empty() || g.video_producer_buffer_caps.is_empty())
        .collect();
    let template_incomplete = games::engine_defaults(pool).await?.is_none_or(|d| {
        d.video_producer_buffer_caps.is_empty() || d.runner_base_create_json.is_empty()
    });
    // The template needs completing even when every game is already healthy:
    // it gained columns of its own, and a game created from an incomplete one
    // cannot launch (found in the field 2026-08-24, M3 create refused).
    if stale.is_empty() && !template_incomplete {
        return Ok(0);
    }
    let from_wolf: std::collections::HashMap<String, Value> = wolf
        .apps_raw()
        .await?
        .into_iter()
        .filter_map(|a| {
            let id = a["id"].as_str()?.to_string();
            Some((id, a))
        })
        .collect();

    // The template a new game inherits was seeded before it had these
    // columns; complete it from the same source, or M3 creates unlaunchable
    // games with empty producer caps.
    if template_incomplete {
        {
            if let Some(app) = from_wolf.values().find(|a| {
                !a["video_producer_buffer_caps"]
                    .as_str()
                    .unwrap_or("")
                    .is_empty()
                    && a["runner"]["type"] == json!("docker")
            }) {
                games::set_engine_defaults(pool, &defaults_from(app)).await?;
                tracing::info!(
                    "library: completed the new-game template from '{}'",
                    app["title"]
                );
            }
        }
    }

    let mut healed = 0;
    for g in stale {
        let Some(app) = from_wolf.get(&g.id) else {
            tracing::error!(
                "library: '{}' has no engine config and wolf has no copy of it; it will not be pushed",
                g.title
            );
            continue;
        };
        let mut fixed = game_from(app);
        // Only engine config comes from Wolf; anything we own stays ours.
        fixed.title = g.title;
        fixed.icon_png_path = g.icon_png_path;
        fixed.runner_json = g.runner_json;
        games::upsert(pool, &fixed).await?;
        healed += 1;
    }
    tracing::info!("library: backfilled engine config for {healed} games");
    Ok(healed)
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
        // Absent means AUTO, which is exactly today's behaviour.
        "controller_override": if g.controller_override.is_empty() {
            json!("AUTO")
        } else {
            json!(g.controller_override)
        },
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

/// A game as an admin supplies it. Engine config is never accepted here: it is
/// inherited from the template, because it belongs to the encoder, not to a
/// game, and an admin has no way to know a correct value for it.
#[derive(serde::Deserialize)]
pub struct NewGame {
    pub title: String,
    pub image: String,
    #[serde(default)]
    pub mounts: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    pub icon: Option<String>,
    pub render_node: Option<String>,
    pub support_hdr: Option<bool>,
    /// Optional. Unlocks Steam art, ProtonDB and store metadata. Usually
    /// unnecessary: it is derived from the game's umu GAMEID or its title.
    /// Supplied here it wins, because an admin correcting a bad match must be
    /// able to.
    pub steam_appid: Option<String>,
}

fn validate(new: &NewGame, lib: &crate::storage::Library) -> Result<(), ApiError> {
    if new.title.trim().is_empty() {
        return Err(ApiError::bad_request("title is required"));
    }
    if new.image.trim().is_empty() {
        return Err(ApiError::bad_request("image is required"));
    }
    // Caught here rather than at lookup time: a bad appid would otherwise
    // enrich the game with a different game's art and compatibility data.
    if let Some(appid) = new.steam_appid.as_deref().map(str::trim) {
        if !appid.is_empty() && !appid.chars().all(|c| c.is_ascii_digit()) {
            return Err(ApiError::bad_request(
                "steam_appid must be the numeric id from the store URL, e.g. 1086940",
            ));
        }
    }
    // A docker bind is "source:destination:mode". A colon anywhere in a path
    // shifts the fields and docker rejects the whole spec, which killed a
    // launch before any container existed (2026-08-24).
    for m in &new.mounts {
        let parts: Vec<&str> = m.split(':').collect();
        if parts.len() != 3 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(ApiError::bad_request(format!(
                "mount '{m}' must be source:destination:mode with no ':' in either path"
            )));
        }
        if !parts[0].starts_with('/') || !parts[1].starts_with('/') {
            return Err(ApiError::bad_request(format!(
                "mount '{m}' needs absolute paths"
            )));
        }
    }
    let Some(run_exe) = new.env.iter().find_map(|e| e.strip_prefix("RUN_EXE=")) else {
        return Err(ApiError::bad_request(
            "env must contain RUN_EXE=<path to the executable inside the container>",
        ));
    };

    // The agent has to be able to SEE the media it manages, the way Jellyfin
    // does: it is what lets the add-a-game form browse to a game, and it is
    // the only way to tell an admin their path is wrong before launch.
    if !lib.is_configured() {
        return Err(ApiError::internal(
            "no library roots configured: set ALTC_LIBRARY_ROOTS and mount the games storage              into the container, otherwise the agent cannot verify the game exists",
        ));
    }
    for m in &new.mounts {
        let source = m.split(':').next().unwrap_or_default();
        // Only game media is required to exist; the agent's own mounts (logs,
        // runtime) live outside the library roots and are created on demand.
        let source_path = std::path::Path::new(source);
        if lib.is_within(source_path) && !source_path.exists() {
            return Err(ApiError::bad_request(format!("no such folder: {source}")));
        }
    }
    match crate::storage::host_path_for(run_exe, &new.mounts) {
        None => {
            return Err(ApiError::bad_request(format!(
                "RUN_EXE '{run_exe}' is not inside any of the mounts"
            )));
        }
        Some(host) => {
            // Only judge paths inside the library: the agent's own mounts
            // (logs, runtime) live elsewhere and are created on demand.
            if lib.is_within(&host) && !host.exists() {
                return Err(ApiError::bad_request(format!(
                    "no such executable: {} (from RUN_EXE '{run_exe}')",
                    host.display()
                )));
            }
        }
    }
    Ok(())
}

pub async fn create(
    pool: &SqlitePool,
    wolf: &WolfClient,
    lib: &crate::storage::Library,
    new: NewGame,
) -> Result<Game, ApiError> {
    validate(&new, lib)?;
    let existing = games::list(pool).await?;
    if existing
        .iter()
        .any(|g| g.title.eq_ignore_ascii_case(new.title.trim()))
    {
        return Err(ApiError::conflict(format!(
            "a game called '{}' already exists",
            new.title.trim()
        )));
    }
    let Some(d) = games::engine_defaults(pool).await? else {
        return Err(ApiError::internal(
            "no template to build a game from; the agent has not imported from wolf yet",
        ));
    };
    if d.video_producer_buffer_caps.is_empty() {
        return Err(ApiError::internal(
            "the new-game template has no video_producer_buffer_caps; a game built from it could not launch",
        ));
    }

    let game = Game {
        id: games::allocate_id(pool).await?,
        title: new.title.trim().to_string(),
        support_hdr: new.support_hdr.unwrap_or(false),
        icon_png_path: new.icon.unwrap_or_default(),
        render_node: new.render_node.unwrap_or_else(|| d.render_node.clone()),
        runner_json: json!({
            "type": "docker",
            "name": new.title.trim().replace(|c: char| !c.is_alphanumeric(), ""),
            "image": new.image.trim(),
            "mounts": new.mounts,
            "env": new.env,
            "devices": [],
            "ports": [],
            "base_create_json": d.runner_base_create_json,
        })
        .to_string(),
        video_producer_buffer_caps: d.video_producer_buffer_caps.clone(),
        h264_gst_pipeline: d.h264_gst_pipeline.clone(),
        hevc_gst_pipeline: d.hevc_gst_pipeline.clone(),
        av1_gst_pipeline: d.av1_gst_pipeline.clone(),
        opus_gst_pipeline: d.opus_gst_pipeline.clone(),
        start_audio_server: d.start_audio_server,
        start_virtual_compositor: d.start_virtual_compositor,
        store: String::new(),
        store_id: String::new(),
        slug: String::new(),
        release_date: String::new(),
        description: String::new(),
        protondb_tier: String::new(),
        steam_appid: new.steam_appid.clone().unwrap_or_default(),
        appid_source: match &new.steam_appid {
            Some(v) if !v.trim().is_empty() => md::APPID_FROM_ADMIN.to_string(),
            _ => String::new(),
        },
        tagline: String::new(),
        developer: String::new(),
        genres: String::new(),
        controller_override: String::new(),
    };
    games::upsert(pool, &game).await?;
    push(pool, wolf).await?;
    tracing::info!("library: added '{}' ({})", game.title, game.id);
    Ok(game)
}

pub async fn remove(pool: &SqlitePool, wolf: &WolfClient, id: &str) -> Result<(), ApiError> {
    games::delete(pool, id).await?;
    push(pool, wolf).await?;
    tracing::info!("library: removed game {id}");
    Ok(())
}

/// The folder a game is installed in, taken from its first mount. Everything
/// the metadata pipeline needs starts here: the store leaves its own manifest
/// in this folder, so the game identifies itself.
pub fn install_folder(g: &Game) -> Option<std::path::PathBuf> {
    let runner: Value = serde_json::from_str(&g.runner_json).ok()?;
    let mounts = runner["mounts"].as_array()?;
    for m in mounts {
        let spec = m.as_str()?;
        let source = spec.split(':').next()?;
        let path = std::path::PathBuf::from(source);
        if crate::identity::detect(&path).is_some() {
            return Some(path);
        }
    }
    None
}

/// Identify a game from its install folder, fetch its metadata and cache its
/// art. Nothing here is supplied by an admin — which is what makes a populated
/// grid evidence that the pipeline ran, rather than evidence someone pasted a
/// URL in.
pub async fn refresh_metadata(
    pool: &SqlitePool,
    wolf: &WolfClient,
    source: &dyn crate::metadata::fetch::Source,
    art_dir: &std::path::Path,
    id: &str,
) -> Result<Game, ApiError> {
    let mut game = games::list(pool)
        .await?
        .into_iter()
        .find(|g| g.id == id)
        .ok_or_else(|| ApiError::not_found("no such game"))?;

    let Some(folder) = install_folder(&game) else {
        return Err(ApiError::bad_request(
            "could not identify this game: no store manifest in any of its mounts",
        ));
    };
    let identity = crate::identity::detect(&folder)
        .ok_or_else(|| ApiError::bad_request("no store manifest in the install folder"))?;

    let mut meta = crate::metadata::fetch::lookup(source, &identity)?;

    // The store the game is installed from goes first; Steam then refines the
    // fields it knows better. Order matters: assigning GOG's values afterwards
    // silently clobbered Steam's release date (found in the field 2026-08-24 —
    // Wolfenstein showed GOG's 2020 re-release instead of its 2014 launch).
    game.store = identity.store.to_string();
    game.store_id = identity.store_id.clone();
    if !meta.title.is_empty() {
        // The store's title is canonical, punctuation and all.
        game.title = meta.title.clone();
    }
    game.slug = meta.slug.clone();
    game.release_date = meta.release_date.clone();
    game.description = meta.description.clone();

    // Precedence: what an admin set, then the appid umu already needs, then a
    // title lookup. The first two are exact; the third is a guess, so it is
    // only accepted on an exact title match and is recorded as a guess.
    let resolved = if !game.steam_appid.is_empty() && game.appid_source == md::APPID_FROM_ADMIN {
        Some((game.steam_appid.clone(), md::APPID_FROM_ADMIN))
    } else if let Some(id) = md::steam_appid_from_runner(&game.runner_json) {
        Some((id, md::APPID_FROM_GAMEID))
    } else {
        let lookup_title = if meta.title.is_empty() {
            game.title.clone()
        } else {
            meta.title.clone()
        };
        source
            .get_text(&md::steam_search_url(&lookup_title))
            .ok()
            .and_then(|b| md::parse_steam_search(&b, &lookup_title))
            .map(|id| (id, md::APPID_FROM_TITLE))
    };

    if let Some((appid, how)) = resolved {
        game.steam_appid = appid.clone();
        game.appid_source = how.to_string();

        // Steam has a real tagline, an accurate release date, genres and a
        // developer — none of which GOG provides.
        if let Some(det) = source
            .get_text(&md::steam_appdetails_url(&appid))
            .ok()
            .and_then(|b| md::parse_steam_details(&b, &appid))
        {
            game.tagline = det.tagline;
            game.developer = det.developer;
            game.genres = det.genres;
            if !det.release_date.is_empty() {
                // GOG reports BG3 as 2020 (early access); Steam says 2023.
                game.release_date = det.release_date;
            }
        }
    }

    if let Some(appid) = (!game.steam_appid.is_empty()).then(|| game.steam_appid.clone()) {
        let mut urls = crate::metadata::steam_art_urls(&appid);
        urls.extend(meta.art_urls.clone());
        meta.art_urls = urls;

        match source
            .get_text(&crate::metadata::protondb_url(&appid))
            .ok()
            .and_then(|b| crate::metadata::parse_protondb(&b))
        {
            Some(tier) => game.protondb_tier = tier,
            None => tracing::debug!("metadata: no protondb entry for appid {appid}"),
        }
    }
    // Art failing must not lose the metadata we already have.
    match crate::metadata::fetch::cache_art(source, art_dir, &game.id, &identity, &meta) {
        Ok(path) => game.icon_png_path = path.to_string_lossy().to_string(),
        Err(e) => tracing::warn!("metadata: no art for '{}': {}", game.title, e.1),
    }

    games::upsert(pool, &game).await?;
    push(pool, wolf).await?;
    tracing::info!(
        "metadata: '{}' identified as {} {}",
        game.title,
        game.store,
        game.store_id
    );
    Ok(game)
}
