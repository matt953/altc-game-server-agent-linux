use crate::wolf::WolfClient;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Clone, Debug, serde::Serialize)]
pub struct LaunchEvent {
    pub client_id: String,
    pub app_id: Option<String>,
    pub state: &'static str,
}

#[derive(Clone)]
pub struct EventHub {
    tx: broadcast::Sender<LaunchEvent>,
    // Only StreamSession carries app_id; remember it for the follow-up events.
    apps: Arc<Mutex<HashMap<String, String>>>,
}

impl EventHub {
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(256).0,
            apps: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LaunchEvent> {
        self.tx.subscribe()
    }

    pub fn publish(&self, kind: &str, data: &Value) {
        let Some((client_id, state)) = translate(kind, data) else {
            return;
        };
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
        }
        let _ = self.tx.send(LaunchEvent {
            client_id,
            app_id,
            state,
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
fn translate(kind: &str, data: &Value) -> Option<(String, &'static str)> {
    let id = |key: &str| data.get(key).map(stringify_id);
    match kind {
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

// Reconnects forever: Wolf restarting must not kill the bridge.
// Logs only on transitions so a long outage doesn't spam.
pub fn spawn_bridge(wolf: WolfClient, hub: EventHub) {
    tokio::spawn(async move {
        let mut was_connected = false;
        loop {
            let connected = Arc::new(Mutex::new(false));
            let flag = connected.clone();
            let result = wolf
                .stream_events(
                    move || {
                        *flag.lock().unwrap() = true;
                        tracing::info!("wolf event bridge connected");
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
