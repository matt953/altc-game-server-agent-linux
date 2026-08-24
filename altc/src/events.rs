use crate::docker::{DockerClient, RunnerUpdate};
use crate::wolf::WolfClient;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Clone, Debug, serde::Serialize)]
pub struct LaunchEvent {
    pub client_id: String,
    pub app_id: Option<String>,
    pub state: &'static str,
    /// 0-100 once the runner starts reporting; absent for coarse Wolf states.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Coarse states carry a percentage too, so a client always has one number to
/// render even before the runner starts reporting.
fn coarse_percent(state: &str) -> Option<u8> {
    match state {
        "connecting" => Some(1),
        "launching" => Some(3),
        "container_started" => Some(5),
        "stopped" => Some(0),
        _ => None,
    }
}

#[derive(Clone)]
pub struct EventHub {
    tx: broadcast::Sender<LaunchEvent>,
    // Only StreamSession carries app_id; remember it for the follow-up events.
    apps: Arc<Mutex<HashMap<String, String>>>,
    // Wolf fires ResumeStream during session setup too; only a real un-pause counts.
    paused: Arc<Mutex<HashSet<String>>>,
    docker: DockerClient,
    // last percent per client: progress must never appear to go backwards
    last_pct: Arc<Mutex<HashMap<String, u8>>>,
    // containers already being followed, so a repeat event doesn't double up
    followed: Arc<Mutex<HashSet<String>>>,
}

impl EventHub {
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(256).0,
            apps: Arc::new(Mutex::new(HashMap::new())),
            paused: Arc::new(Mutex::new(HashSet::new())),
            docker: DockerClient::from_env(),
            last_pct: Arc::new(Mutex::new(HashMap::new())),
            followed: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LaunchEvent> {
        self.tx.subscribe()
    }

    pub fn publish(&self, kind: &str, data: &Value) {
        let Some((client_id, state)) = translate(kind, data) else {
            tracing::debug!("wolf event ignored: {kind}");
            return;
        };
        match state {
            "paused" => {
                self.paused.lock().unwrap().insert(client_id.clone());
            }
            "resumed" if !self.paused.lock().unwrap().remove(&client_id) => return,
            _ => {}
        }
        let app_id = match data.get("app_id").and_then(|v| v.as_str()) {
            Some(a) => {
                self.apps
                    .lock()
                    .unwrap()
                    .insert(client_id.clone(), a.to_string());
                Some(a.to_string())
            }
            None => self.apps.lock().unwrap().get(&client_id).cloned(),
        };
        if state == "stopped" {
            self.apps.lock().unwrap().remove(&client_id);
            self.paused.lock().unwrap().remove(&client_id);
        }
        // Once the container exists we can read the runner's own progress.
        if state == "container_started" {
            if let Some(cid) = data.get("container_id").and_then(|v| v.as_str()) {
                self.follow_container(cid.to_string(), client_id.clone(), app_id.clone());
            }
        }

        let pct = coarse_percent(state);
        if let Some(p) = pct {
            if !self.advances(&client_id, p) {
                return;
            }
        }
        let _ = self.tx.send(LaunchEvent {
            client_id,
            app_id,
            state,
            percent: pct,
            error: None,
        });
    }

    /// True when this percentage is an advance for the client. A new session
    /// (percent 0/1) resets the ratchet.
    fn advances(&self, client_id: &str, pct: u8) -> bool {
        let mut map = self.last_pct.lock().unwrap();
        let prev = map.get(client_id).copied();
        if pct <= 1 {
            map.insert(client_id.to_string(), pct);
            return true;
        }
        match prev {
            Some(p) if pct <= p => false,
            _ => {
                map.insert(client_id.to_string(), pct);
                true
            }
        }
    }

    fn follow_container(&self, container_id: String, client_id: String, app_id: Option<String>) {
        if !self.followed.lock().unwrap().insert(container_id.clone()) {
            return;
        }
        let (docker, tx, followed) = (self.docker.clone(), self.tx.clone(), self.followed.clone());
        let hub = self.clone();
        tokio::spawn(async move {
            docker
                .follow_progress(&container_id, |u| {
                    let ev = match u {
                        RunnerUpdate::Percent(p) if !hub.advances(&client_id, p) => return,
                        RunnerUpdate::Percent(p) => LaunchEvent {
                            client_id: client_id.clone(),
                            app_id: app_id.clone(),
                            state: if p >= 100 { "running" } else { "starting" },
                            percent: Some(p),
                            error: None,
                        },
                        RunnerUpdate::Failed(reason) => LaunchEvent {
                            client_id: client_id.clone(),
                            app_id: app_id.clone(),
                            state: "failed",
                            percent: None,
                            error: Some(reason),
                        },
                    };
                    let _ = tx.send(ev);
                })
                .await;
            followed.lock().unwrap().remove(&container_id);
        });
    }
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

// Wolf event -> (client_id, client-facing state). Unmapped events are dropped:
// Wolf's raw payloads carry session secrets and must never be proxied through.
// Wire names are namespaced ("wolf::core::events::StreamSession"); match the leaf.
fn translate(kind: &str, data: &Value) -> Option<(String, &'static str)> {
    let id = |key: &str| data.get(key).map(stringify_id);
    match kind.rsplit("::").next()? {
        "StreamSession" => Some((id("client_id")?, "connecting")),
        "StartRunner" => Some((id("session_id")?, "launching")),
        "DockerContainerCreated" => Some((id("session_id")?, "container_started")),
        "DockerContainerStopped" => Some((id("session_id")?, "stopped")),
        "StopStreamEvent" => Some((id("session_id")?, "stopped")),
        "PauseStreamEvent" => Some((id("session_id")?, "paused")),
        "ResumeStreamEvent" => Some((id("session_id")?, "resumed")),
        _ => None,
    }
}

// session_id is a number in some events, a string in others.
fn stringify_id(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// Adopt whatever Wolf already had on first ever run, then make its list match
// ours. Failure here must not kill the bridge: events still matter.
async fn sync_library(pool: &sqlx::SqlitePool, wolf: &WolfClient) {
    if let Err(e) = crate::library::import_if_empty(pool, wolf).await {
        tracing::error!("library import failed: {}", e.1);
        return;
    }
    if let Err(e) = crate::library::push(pool, wolf).await {
        tracing::error!("library push failed: {}", e.1);
    }
}

// Reconnects forever: Wolf restarting must not kill the bridge.
// Logs only on transitions so a long outage doesn't spam.
pub fn spawn_bridge(wolf: WolfClient, hub: EventHub, pool: sqlx::SqlitePool) {
    tokio::spawn(async move {
        let mut was_connected = false;
        loop {
            let connected = Arc::new(Mutex::new(false));
            let flag = connected.clone();
            let (sync_wolf, sync_pool) = (wolf.clone(), pool.clone());
            let result = wolf
                .stream_events(
                    move || {
                        *flag.lock().unwrap() = true;
                        tracing::info!("wolf event bridge connected");
                        // Wolf holds the library in memory only, so every
                        // (re)connect is also "Wolf lost the apps, re-send".
                        let (w, p) = (sync_wolf.clone(), sync_pool.clone());
                        tokio::spawn(async move { sync_library(&p, &w).await });
                    },
                    |kind, data| hub.publish(kind, data),
                )
                .await;
            let opened = *connected.lock().unwrap();
            match (opened || was_connected, result) {
                (true, Err(e)) => tracing::warn!("wolf event stream lost: {e}"),
                (true, Ok(())) => tracing::warn!("wolf event stream closed, reconnecting"),
                (false, Err(e)) => tracing::debug!("wolf event stream unavailable: {e}"),
                (false, Ok(())) => {}
            }
            was_connected = false;
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    });
}
