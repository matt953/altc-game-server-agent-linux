use altc_api::{db, events::EventHub, routes, state::AppState, wolf::WolfClient};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;

async fn setup() -> (Router, EventHub, String, i64) {
    let pool = db::init_memory().await;
    let owner_id = db::users::insert(&pool, "owner", "owner").await.unwrap();
    let token = db::tokens::issue(&pool, owner_id, "t").await.unwrap();
    let events = EventHub::new();
    let wolf = WolfClient::new("/nonexistent/wolf.sock".into());
    let app = routes::router(AppState {
        pool: pool.clone(),
        wolf,
        events: events.clone(),
    });
    (app, events, token, owner_id)
}

// Collects SSE payloads for a short window, then gives up.
async fn collect_events(
    app: &Router,
    token: &str,
    hub: &EventHub,
    emit: Vec<(&str, Value)>,
) -> Vec<Value> {
    let res = app
        .clone()
        .oneshot(
            Request::get("/api/v1/events")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let mut stream = res.into_body().into_data_stream();
    for (kind, data) in emit {
        hub.publish(kind, &data);
    }

    let mut out = Vec::new();
    let deadline = tokio::time::sleep(Duration::from_millis(400));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            chunk = stream.next() => {
                let Some(Ok(bytes)) = chunk else { break };
                for line in String::from_utf8_lossy(&bytes).lines() {
                    if let Some(d) = line.strip_prefix("data: ") {
                        if let Ok(v) = serde_json::from_str::<Value>(d) {
                            out.push(v);
                        }
                    }
                }
            }
        }
    }
    out
}

#[tokio::test]
async fn events_require_auth() {
    let (app, _, _, _) = setup().await;
    let res = app
        .oneshot(Request::get("/api/v1/events").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wolf_events_translate_to_launch_states() {
    let (app, hub, owner, _) = setup().await;
    let got = collect_events(
        &app,
        &owner,
        &hub,
        vec![
            ("wolf::core::events::StreamSession", json!({"client_id": "dev1", "app_id": "639825709", "aes_key": "SECRET", "aes_iv": "SECRET"})),
            ("wolf::core::events::StartRunner", json!({"session_id": "dev1"})),
            ("wolf::core::events::DockerContainerCreated", json!({"session_id": "dev1", "container_id": "abc"})),
            ("wolf::core::events::DockerContainerStopped", json!({"session_id": "dev1"})),
        ],
    )
    .await;

    let states: Vec<&str> = got.iter().map(|e| e["state"].as_str().unwrap()).collect();
    assert_eq!(
        states,
        vec!["connecting", "launching", "container_started", "stopped"]
    );
    // app_id is remembered from StreamSession for the follow-up events.
    assert_eq!(got[1]["app_id"], "639825709");
    assert_eq!(got[2]["app_id"], "639825709");

    let raw = serde_json::to_string(&got).unwrap();
    assert!(
        !raw.contains("SECRET"),
        "session secrets must never be forwarded"
    );
    assert!(!raw.contains("aes"));
    assert!(!raw.contains("container_id"));
}

#[tokio::test]
async fn numeric_session_ids_are_stringified() {
    let (app, hub, owner, _) = setup().await;
    let got = collect_events(
        &app,
        &owner,
        &hub,
        vec![(
            "StopStreamEvent",
            json!({"session_id": 11720759129636514154u64}),
        )],
    )
    .await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0]["client_id"], "11720759129636514154");
    assert_eq!(got[0]["state"], "stopped");
}

#[tokio::test]
async fn unmapped_events_are_dropped() {
    let (app, hub, owner, _) = setup().await;
    let got = collect_events(
        &app,
        &owner,
        &hub,
        vec![
            (
                "PairSignal",
                json!({"client_ip": "10.0.0.1", "host_ip": "10.0.0.2"}),
            ),
            (
                "wolf::core::events::RTPVideoPingEvent",
                json!({"client_ip": "10.0.0.1"}),
            ),
            (
                "VideoSession",
                json!({"session_id": "dev1", "aes_key": "SECRET"}),
            ),
        ],
    )
    .await;
    assert!(got.is_empty(), "only mapped launch events should surface");
}

#[tokio::test]
async fn members_only_see_their_own_devices() {
    let pool = db::init_memory().await;
    let owner_id = db::users::insert(&pool, "owner", "owner").await.unwrap();
    let owner = db::tokens::issue(&pool, owner_id, "t").await.unwrap();
    let dave_id = db::users::insert(&pool, "dave", "member").await.unwrap();
    let dave = db::tokens::issue(&pool, dave_id, "t").await.unwrap();
    db::devices::upsert(&pool, dave_id, "dave-dev", &None)
        .await
        .unwrap();

    let events = EventHub::new();
    let app = routes::router(AppState {
        pool: pool.clone(),
        wolf: WolfClient::new("/nonexistent/wolf.sock".into()),
        events: events.clone(),
    });

    let emit = vec![
        (
            "wolf::core::events::StartRunner",
            json!({"session_id": "dave-dev"}),
        ),
        (
            "wolf::core::events::StartRunner",
            json!({"session_id": "someone-else"}),
        ),
    ];

    let dave_saw = collect_events(&app, &dave, &events, emit.clone()).await;
    assert_eq!(dave_saw.len(), 1);
    assert_eq!(dave_saw[0]["client_id"], "dave-dev");

    let owner_saw = collect_events(&app, &owner, &events, emit).await;
    assert_eq!(owner_saw.len(), 2, "admins see every session");
}

#[tokio::test]
async fn spurious_resume_at_session_start_is_suppressed() {
    let (app, hub, owner, _) = setup().await;
    // Real launch order observed in production 2026-08-23: Wolf fires
    // ResumeStream during setup, before the runner even starts.
    let got = collect_events(
        &app,
        &owner,
        &hub,
        vec![
            (
                "wolf::core::events::StreamSession",
                json!({"client_id": "d1", "app_id": "a1"}),
            ),
            (
                "wolf::core::events::ResumeStreamEvent",
                json!({"session_id": "d1"}),
            ),
            (
                "wolf::core::events::StartRunner",
                json!({"session_id": "d1"}),
            ),
        ],
    )
    .await;
    let states: Vec<&str> = got.iter().map(|e| e["state"].as_str().unwrap()).collect();
    assert_eq!(states, vec!["connecting", "launching"]);
}

#[tokio::test]
async fn genuine_pause_resume_pair_surfaces() {
    let (app, hub, owner, _) = setup().await;
    let got = collect_events(
        &app,
        &owner,
        &hub,
        vec![
            (
                "wolf::core::events::PauseStreamEvent",
                json!({"session_id": "d1"}),
            ),
            (
                "wolf::core::events::ResumeStreamEvent",
                json!({"session_id": "d1"}),
            ),
        ],
    )
    .await;
    let states: Vec<&str> = got.iter().map(|e| e["state"].as_str().unwrap()).collect();
    assert_eq!(states, vec!["paused", "resumed"]);
}
