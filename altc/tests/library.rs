use altc_api::{db, library, wolf::WolfClient};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

type Apps = Arc<Mutex<Vec<Value>>>;

// A Wolf that actually holds state, so add/delete round-trips are observable.
// Mirrors the real one: apps/add requires an id and stores it verbatim.
fn fake_wolf(apps: Apps) -> Router {
    Router::new()
        .route(
            "/api/v1/apps",
            get(|State(a): State<Apps>| async move {
                Json(json!({"success": true, "apps": *a.lock().unwrap()}))
            }),
        )
        .route(
            "/api/v1/apps/add",
            post(|State(a): State<Apps>, Json(app): Json<Value>| async move {
                for required in [
                    "id",
                    "title",
                    "h264_gst_pipeline",
                    "hevc_gst_pipeline",
                    "av1_gst_pipeline",
                    "opus_gst_pipeline",
                    "render_node",
                    "video_producer_buffer_caps",
                    "start_virtual_compositor",
                    "start_audio_server",
                ] {
                    if app.get(required).is_none() {
                        return Json(
                            json!({"success": false, "error": format!("Field named '{required}' not found.")}),
                        );
                    }
                }
                a.lock().unwrap().push(app);
                Json(json!({"success": true}))
            }),
        )
        .route(
            "/api/v1/apps/delete",
            post(|State(a): State<Apps>, Json(body): Json<Value>| async move {
                let id = body["id"].as_str().unwrap_or_default().to_string();
                a.lock().unwrap().retain(|x| x["id"] != json!(id));
                Json(json!({"success": true}))
            }),
        )
        .with_state(apps)
}

fn seed_app(id: &str, title: &str) -> Value {
    json!({
        "id": id, "title": title, "support_hdr": false, "icon_png_path": "",
        "render_node": "/dev/dri/renderD128",
        "h264_gst_pipeline": "h264-template", "hevc_gst_pipeline": "hevc-template",
        "av1_gst_pipeline": "av1-template", "opus_gst_pipeline": "opus-template",
        "video_producer_buffer_caps": "video/x-raw, format=NV12",
        "start_virtual_compositor": true, "start_audio_server": true,
        "runner": {"type": "docker", "image": "ghcr.io/matt953/wolf-runner:v7",
                   "mounts": ["/games/bg3:/games/bg3:rw"], "env": ["RUN_EXE=/games/bg3/bg3.exe"],
                   "base_create_json": "{\"HostConfig\":{\"IpcMode\":\"host\"}}"},
    })
}

async fn start(test: &str, seed: Vec<Value>) -> (WolfClient, Apps) {
    let apps: Apps = Arc::new(Mutex::new(seed));
    let socket = PathBuf::from(std::env::temp_dir())
        .join(format!("altc-lib-{}-{test}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let router = fake_wolf(apps.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (WolfClient::new(socket), apps)
}

#[tokio::test]
async fn adopts_wolfs_apps_then_pushes_them_back_identically() {
    let pool = db::init_memory().await;
    let (wolf, apps) = start(
        "roundtrip",
        vec![
            seed_app("578802895", "Baldur's Gate 3"),
            seed_app("465409800", "Wolfenstein: The New Order"),
        ],
    )
    .await;

    assert_eq!(library::import_if_empty(&pool, &wolf).await.unwrap(), 2);

    // Wolf loses everything on restart; the agent must restore it unprompted.
    apps.lock().unwrap().clear();
    assert_eq!(library::push(&pool, &wolf).await.unwrap(), 2);

    let restored = apps.lock().unwrap().clone();
    assert_eq!(restored.len(), 2);
    let ids: Vec<&str> = restored.iter().map(|a| a["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"578802895"), "ids must survive: {ids:?}");
    assert!(ids.contains(&"465409800"), "ids must survive: {ids:?}");

    // Engine config is not per-game, but Wolf still demands it on every app.
    let one = &restored[0];
    assert_eq!(one["h264_gst_pipeline"], "h264-template");
    assert_eq!(one["opus_gst_pipeline"], "opus-template");
    assert_eq!(one["runner"]["image"], "ghcr.io/matt953/wolf-runner:v7");
    // Wolf's reflector dropped this until 2026-08-24; an app without it builds
    // a malformed video pipeline and never launches.
    assert_eq!(
        one["video_producer_buffer_caps"],
        "video/x-raw, format=NV12"
    );
    assert_eq!(one["runner"]["mounts"][0], "/games/bg3:/games/bg3:rw");
}

#[tokio::test]
async fn a_colon_in_a_title_survives_the_round_trip() {
    let pool = db::init_memory().await;
    let (wolf, apps) = start(
        "colon",
        vec![seed_app("465409800", "Wolfenstein: The New Order")],
    )
    .await;
    library::import_if_empty(&pool, &wolf).await.unwrap();
    apps.lock().unwrap().clear();
    library::push(&pool, &wolf).await.unwrap();
    assert_eq!(
        apps.lock().unwrap()[0]["title"],
        "Wolfenstein: The New Order"
    );
}

// Wolf's runner is polymorphic: Test ball is {"type":"process","run_cmd":...}
// with none of the docker fields. Storing the whole blob must preserve that.
#[tokio::test]
async fn a_process_runner_survives_the_round_trip() {
    let pool = db::init_memory().await;
    let mut ball = seed_app("249285395", "Test ball");
    ball["runner"] = json!({"type": "process", "run_cmd": "sh -c \"while :; do sleep 10; done\""});
    let (wolf, apps) = start("process", vec![ball.clone()]).await;
    library::import_if_empty(&pool, &wolf).await.unwrap();
    apps.lock().unwrap().clear();
    library::push(&pool, &wolf).await.unwrap();
    assert_eq!(apps.lock().unwrap()[0]["runner"], ball["runner"]);
}

// Regression 2026-08-24: pushing used one shared engine_defaults row, so an
// app that overrides its pipelines (Test ball) came back with another app's.
#[tokio::test]
async fn each_apps_engine_overrides_survive_a_push() {
    let pool = db::init_memory().await;
    let mut ball = seed_app("249285395", "Test ball");
    ball["h264_gst_pipeline"] = json!("videotestsrc pattern=ball");
    ball["hevc_gst_pipeline"] = json!("videotestsrc pattern=ball");
    ball["av1_gst_pipeline"] = json!("videotestsrc pattern=ball");
    ball["opus_gst_pipeline"] = json!("audiotestsrc wave=ticks");
    ball["start_audio_server"] = json!(false);
    ball["start_virtual_compositor"] = json!(false);

    let bg3 = seed_app("578802895", "Baldur's Gate 3");
    let (wolf, apps) = start("overrides", vec![bg3.clone(), ball.clone()]).await;
    library::import_if_empty(&pool, &wolf).await.unwrap();
    apps.lock().unwrap().clear();
    library::push(&pool, &wolf).await.unwrap();

    let out = apps.lock().unwrap().clone();
    let find = |id: &str| out.iter().find(|a| a["id"] == json!(id)).unwrap().clone();
    let got_ball = find("249285395");
    assert_eq!(got_ball["h264_gst_pipeline"], ball["h264_gst_pipeline"]);
    assert_eq!(got_ball["opus_gst_pipeline"], ball["opus_gst_pipeline"]);
    assert_eq!(got_ball["start_audio_server"], json!(false));
    assert_eq!(got_ball["start_virtual_compositor"], json!(false));

    let got_bg3 = find("578802895");
    assert_eq!(got_bg3["h264_gst_pipeline"], bg3["h264_gst_pipeline"]);
    assert_eq!(got_bg3["start_audio_server"], json!(true));
}

#[tokio::test]
async fn import_runs_once_not_on_every_reconnect() {
    let pool = db::init_memory().await;
    let (wolf, _apps) = start("once", vec![seed_app("1", "One")]).await;
    assert_eq!(library::import_if_empty(&pool, &wolf).await.unwrap(), 1);
    // Second connect: Wolf may hold anything; our library is already authoritative.
    assert_eq!(library::import_if_empty(&pool, &wolf).await.unwrap(), 0);
    assert_eq!(db::games::count(&pool).await.unwrap(), 1);
}

#[tokio::test]
async fn repeated_pushes_do_not_duplicate_apps() {
    let pool = db::init_memory().await;
    let (wolf, apps) = start(
        "idempotent",
        vec![seed_app("1", "One"), seed_app("2", "Two")],
    )
    .await;
    library::import_if_empty(&pool, &wolf).await.unwrap();
    for _ in 0..3 {
        library::push(&pool, &wolf).await.unwrap();
    }
    assert_eq!(
        apps.lock().unwrap().len(),
        2,
        "push must be a replace, not an append"
    );
}

#[tokio::test]
async fn push_of_a_game_without_engine_config_is_refused_not_half_done() {
    let pool = db::init_memory().await;
    let (wolf, apps) = start("nodefaults", vec![]).await;
    // A game with no pipelines: Wolf accepts the payload and then cannot
    // build a session for it, so refuse before sending anything.
    db::games::upsert(
        &pool,
        &db::games::Game {
            id: "9".into(),
            title: "Orphan".into(),
            support_hdr: false,
            icon_png_path: String::new(),
            render_node: "/dev/dri/renderD128".into(),
            runner_json: "{}".into(),
            video_producer_buffer_caps: String::new(),
            h264_gst_pipeline: String::new(),
            hevc_gst_pipeline: String::new(),
            av1_gst_pipeline: String::new(),
            opus_gst_pipeline: String::new(),
            start_audio_server: true,
            start_virtual_compositor: true,
        },
    )
    .await
    .unwrap();
    assert!(library::push(&pool, &wolf).await.is_err());
    assert!(apps.lock().unwrap().is_empty(), "nothing may be sent");
}

// Rows written before per-app engine config are completed from Wolf's own
// copy, not dropped: a game we own but Wolf has never seen must survive.
#[tokio::test]
async fn backfill_completes_old_rows_and_keeps_overrides() {
    let pool = db::init_memory().await;
    let mut ball = seed_app("249285395", "Test ball");
    ball["h264_gst_pipeline"] = json!("videotestsrc pattern=ball");
    ball["hevc_gst_pipeline"] = json!("videotestsrc pattern=ball");
    ball["opus_gst_pipeline"] = json!("audiotestsrc wave=ticks");
    ball["start_audio_server"] = json!(false);
    let (wolf, _apps) = start("backfill", vec![ball.clone(), seed_app("1", "One")]).await;

    // Two rows as the old schema left them: no engine config at all.
    for (id, title) in [("249285395", "Test ball"), ("1", "One")] {
        db::games::upsert(
            &pool,
            &db::games::Game {
                id: id.into(),
                title: title.into(),
                support_hdr: false,
                icon_png_path: String::new(),
                render_node: "/dev/dri/renderD128".into(),
                runner_json: "{\"type\":\"docker\"}".into(),
                video_producer_buffer_caps: String::new(),
                h264_gst_pipeline: String::new(),
                hevc_gst_pipeline: String::new(),
                av1_gst_pipeline: String::new(),
                opus_gst_pipeline: String::new(),
                start_audio_server: true,
                start_virtual_compositor: true,
            },
        )
        .await
        .unwrap();
    }

    assert_eq!(
        library::backfill_engine_config(&pool, &wolf).await.unwrap(),
        2
    );

    let rows = db::games::list(&pool).await.unwrap();
    let ball_row = rows.iter().find(|g| g.id == "249285395").unwrap();
    assert_eq!(ball_row.hevc_gst_pipeline, "videotestsrc pattern=ball");
    assert_eq!(ball_row.opus_gst_pipeline, "audiotestsrc wave=ticks");
    assert!(
        !ball_row.start_audio_server,
        "override must survive backfill"
    );
    assert_eq!(
        ball_row.video_producer_buffer_caps,
        "video/x-raw, format=NV12"
    );
    // Rows are completed in place: nothing was dropped or renamed.
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|g| g.title == "One"));
}

// A game we own that Wolf has never seen keeps its row rather than being
// deleted; it is reported and held back from the push instead.
#[tokio::test]
async fn backfill_keeps_a_game_wolf_does_not_know() {
    let pool = db::init_memory().await;
    let (wolf, _apps) = start("orphan", vec![seed_app("1", "One")]).await;
    db::games::upsert(
        &pool,
        &db::games::Game {
            id: "999".into(),
            title: "Added by hand".into(),
            support_hdr: false,
            icon_png_path: String::new(),
            render_node: "/dev/dri/renderD128".into(),
            runner_json: "{}".into(),
            video_producer_buffer_caps: String::new(),
            h264_gst_pipeline: String::new(),
            hevc_gst_pipeline: String::new(),
            av1_gst_pipeline: String::new(),
            opus_gst_pipeline: String::new(),
            start_audio_server: true,
            start_virtual_compositor: true,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        library::backfill_engine_config(&pool, &wolf).await.unwrap(),
        0
    );
    let rows = db::games::list(&pool).await.unwrap();
    assert_eq!(rows.len(), 1, "the row must not be deleted");
    assert_eq!(rows[0].title, "Added by hand");
}

// M2: identity must not move when a title or its art does. Wolf derives an id
// from hash(icon_png_path + title) when seeding from config, which is why
// Wolfenstein's id changed twice on 2026-08-24 when it was retitled.
#[tokio::test]
async fn renaming_a_game_keeps_its_id_and_its_shares() {
    let pool = db::init_memory().await;
    let (wolf, apps) = start(
        "rename",
        vec![seed_app("465409800", "Wolfenstein - The New Order")],
    )
    .await;
    library::import_if_empty(&pool, &wolf).await.unwrap();

    // Someone is granted access to it before the rename.
    let uid = db::users::insert(&pool, "dave", "member").await.unwrap();
    db::shares::set_for_app(&pool, "465409800", &[uid])
        .await
        .unwrap();

    db::games::rename(
        &pool,
        "465409800",
        "Wolfenstein: The New Order",
        "https://example.com/new-art.png",
    )
    .await
    .unwrap();

    apps.lock().unwrap().clear();
    library::push(&pool, &wolf).await.unwrap();

    let pushed = apps.lock().unwrap().clone();
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0]["id"], "465409800", "id must survive a retitle");
    assert_eq!(pushed[0]["title"], "Wolfenstein: The New Order");
    assert_eq!(
        pushed[0]["icon_png_path"],
        "https://example.com/new-art.png"
    );
    // Shares are keyed by id, so they must still point at the same game.
    assert_eq!(
        db::shares::for_app(&pool, "465409800").await.unwrap(),
        vec![uid]
    );
}

#[tokio::test]
async fn allocated_ids_are_unique_and_fit_a_moonlight_client() {
    let pool = db::init_memory().await;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..32 {
        let id = db::games::allocate_id(&pool).await.unwrap();
        let n: i64 = id.parse().expect("id must be numeric");
        assert!(
            n > 0 && n <= i32::MAX as i64,
            "id {n} must fit signed 32-bit"
        );
        assert!(seen.insert(id.clone()), "allocate_id returned a duplicate");
        // Occupy it, so the next call has to avoid it.
        db::games::upsert(
            &pool,
            &db::games::Game {
                id,
                title: "x".into(),
                support_hdr: false,
                icon_png_path: String::new(),
                render_node: String::new(),
                runner_json: "{}".into(),
                video_producer_buffer_caps: "caps".into(),
                h264_gst_pipeline: "h".into(),
                hevc_gst_pipeline: "h".into(),
                av1_gst_pipeline: "a".into(),
                opus_gst_pipeline: "o".into(),
                start_audio_server: true,
                start_virtual_compositor: true,
            },
        )
        .await
        .unwrap();
    }
}

fn valid_new() -> serde_json::Value {
    json!({
        "title": "Doom Eternal",
        "image": "ghcr.io/matt953/wolf-runner:v7",
        "mounts": ["/mnt/games/doom:/games/doom:rw"],
        "env": ["RUN_EXE=/games/doom/DOOMEternalx64vk.exe", "GAMEID=umu-782330"],
    })
}

// Each test needs its own socket: they run in parallel and would otherwise
// bind the same path and race.
async fn seeded(name: &str) -> (sqlx::SqlitePool, WolfClient, Apps) {
    let pool = db::init_memory().await;
    let (wolf, apps) = start(name, vec![seed_app("1", "Seed")]).await;
    library::import_if_empty(&pool, &wolf).await.unwrap();
    library::backfill_engine_config(&pool, &wolf).await.unwrap();
    (pool, wolf, apps)
}

#[tokio::test]
async fn a_game_added_over_the_api_reaches_wolf_launchable() {
    let (pool, wolf, apps) = seeded("m3-add").await;
    let new: library::NewGame = serde_json::from_value(valid_new()).unwrap();
    let game = library::create(&pool, &wolf, new).await.unwrap();

    let pushed = apps.lock().unwrap().clone();
    let added = pushed
        .iter()
        .find(|a| a["title"] == json!("Doom Eternal"))
        .unwrap();
    assert_eq!(added["id"], json!(game.id));
    // Inherited from the template: an admin cannot know these, and an empty
    // producer caps is the difference between launching and not.
    assert_eq!(
        added["video_producer_buffer_caps"],
        "video/x-raw, format=NV12"
    );
    assert_eq!(added["hevc_gst_pipeline"], "hevc-template");
    assert_eq!(added["runner"]["image"], "ghcr.io/matt953/wolf-runner:v7");
    assert_eq!(
        added["runner"]["env"][0],
        "RUN_EXE=/games/doom/DOOMEternalx64vk.exe"
    );
    // Allocated, not derived from the title.
    assert_ne!(game.id, "1");
}

#[tokio::test]
async fn a_colon_in_a_mount_path_is_refused() {
    let (pool, wolf, apps) = seeded("m3-colon").await;
    let mut body = valid_new();
    body["mounts"] = json!(["/mnt/games/Wolfenstein: The New Order:/games/w:rw"]);
    let new: library::NewGame = serde_json::from_value(body).unwrap();
    let err = library::create(&pool, &wolf, new).await.unwrap_err();
    assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    assert!(err.1.contains("source:destination:mode"), "got: {}", err.1);
    assert_eq!(apps.lock().unwrap().len(), 1, "nothing may be pushed");
}

#[tokio::test]
async fn a_game_without_run_exe_is_refused() {
    let (pool, wolf, _apps) = seeded("m3-runexe").await;
    let mut body = valid_new();
    body["env"] = json!(["GAMEID=umu-1"]);
    let new: library::NewGame = serde_json::from_value(body).unwrap();
    let err = library::create(&pool, &wolf, new).await.unwrap_err();
    assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    assert!(err.1.contains("RUN_EXE"));
}

#[tokio::test]
async fn a_duplicate_title_is_refused() {
    let (pool, wolf, _apps) = seeded("m3-dup").await;
    let new: library::NewGame = serde_json::from_value(valid_new()).unwrap();
    library::create(&pool, &wolf, new).await.unwrap();
    let again: library::NewGame = serde_json::from_value(valid_new()).unwrap();
    let err = library::create(&pool, &wolf, again).await.unwrap_err();
    assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
}

#[tokio::test]
async fn deleting_a_game_removes_it_from_wolf_and_drops_its_shares() {
    let (pool, wolf, apps) = seeded("m3-delete").await;
    let new: library::NewGame = serde_json::from_value(valid_new()).unwrap();
    let game = library::create(&pool, &wolf, new).await.unwrap();
    let uid = db::users::insert(&pool, "dave", "member").await.unwrap();
    db::shares::set_for_app(&pool, &game.id, &[uid])
        .await
        .unwrap();

    library::remove(&pool, &wolf, &game.id).await.unwrap();

    assert!(
        !apps
            .lock()
            .unwrap()
            .iter()
            .any(|a| a["id"] == json!(game.id))
    );
    assert!(
        db::games::list(&pool)
            .await
            .unwrap()
            .iter()
            .all(|g| g.id != game.id)
    );
    // Shares have no foreign key to games, so they must be cleaned explicitly
    // or they would re-attach if the id were ever reused.
    assert!(
        db::shares::for_app(&pool, &game.id)
            .await
            .unwrap()
            .is_empty()
    );
}

// Field-found 2026-08-24: the template completion sat behind an early return
// that fires when no game is stale, so on a healthy library it never ran and
// M3's create refused with "template has no video_producer_buffer_caps".
#[tokio::test]
async fn the_new_game_template_is_completed_even_when_no_game_is_stale() {
    let pool = db::init_memory().await;
    let (wolf, _apps) = start("m3-template", vec![seed_app("1", "Seed")]).await;
    library::import_if_empty(&pool, &wolf).await.unwrap();

    // Every game healthy, but the template predates its own columns.
    assert!(
        db::games::list(&pool)
            .await
            .unwrap()
            .iter()
            .all(|g| !g.video_producer_buffer_caps.is_empty())
    );
    sqlx::query("UPDATE engine_defaults SET video_producer_buffer_caps='', runner_base_create_json='' WHERE id=1")
        .execute(&pool)
        .await
        .unwrap();

    library::backfill_engine_config(&pool, &wolf).await.unwrap();

    let d = db::games::engine_defaults(&pool).await.unwrap().unwrap();
    assert_eq!(d.video_producer_buffer_caps, "video/x-raw, format=NV12");
    // The HostConfig blob a docker runner cannot start without.
    assert_eq!(
        d.runner_base_create_json,
        "{\"HostConfig\":{\"IpcMode\":\"host\"}}"
    );
    // ...and a game can now actually be created from it.
    let new: library::NewGame = serde_json::from_value(valid_new()).unwrap();
    assert!(library::create(&pool, &wolf, new).await.is_ok());
}
