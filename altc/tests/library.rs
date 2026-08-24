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
        "start_virtual_compositor": true, "start_audio_server": true,
        "runner": {"type": "docker", "image": "ghcr.io/matt953/wolf-runner:v7",
                   "mounts": ["/games/bg3:/games/bg3:rw"], "env": ["RUN_EXE=/games/bg3/bg3.exe"]},
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
async fn push_without_engine_defaults_is_refused_rather_than_half_done() {
    let pool = db::init_memory().await;
    let (wolf, apps) = start("nodefaults", vec![]).await;
    // A game with no engine defaults recorded: pushing would build payloads
    // Wolf rejects field-by-field, leaving a partial library.
    db::games::upsert(
        &pool,
        &db::games::Game {
            id: "9".into(),
            title: "Orphan".into(),
            support_hdr: false,
            icon_png_path: String::new(),
            render_node: "/dev/dri/renderD128".into(),
            runner_json: "{}".into(),
        },
    )
    .await
    .unwrap();
    assert!(library::push(&pool, &wolf).await.is_err());
    assert!(apps.lock().unwrap().is_empty(), "nothing may be sent");
}
