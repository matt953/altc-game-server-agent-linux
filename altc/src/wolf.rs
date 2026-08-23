use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::client::legacy::Client;
use hyperlocal::{UnixClientExt, UnixConnector, Uri};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Clone)]
pub struct WolfClient {
    socket: PathBuf,
    http: Client<UnixConnector, Full<Bytes>>,
}

#[derive(Debug)]
pub struct WolfError(pub String);

impl std::fmt::Display for WolfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "wolf socket: {}", self.0)
    }
}

#[derive(Deserialize, serde::Serialize)]
pub struct PendingPair {
    pub pair_secret: String,
    pub client_ip: String,
    pub client_id: String,
}

#[derive(Deserialize, serde::Serialize)]
pub struct PairedClient {
    pub client_id: String,
    pub app_state_folder: String,
}

// Trimmed from Wolf's full app object; pipelines/runner internals never leave the agent.
#[derive(Deserialize, serde::Serialize)]
pub struct WolfApp {
    pub id: String,
    pub title: String,
    pub support_hdr: bool,
    #[serde(default)]
    pub icon_png_path: Option<String>,
}

impl WolfClient {
    pub fn from_env() -> Self {
        let socket = std::env::var("WOLF_SOCKET_PATH").unwrap_or_else(|_| {
            let runtime =
                std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp/sockets".into());
            format!("{runtime}/wolf.sock")
        });
        Self::new(socket.into())
    }

    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            http: Client::unix(),
        }
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, WolfError> {
        let uri: hyper::Uri = Uri::new(&self.socket, path).into();
        let req = hyper::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(
                body.map(|b| b.to_string()).unwrap_or_default(),
            )))
            .map_err(|e| WolfError(e.to_string()))?;
        let res = self
            .http
            .request(req)
            .await
            .map_err(|e| WolfError(e.to_string()))?;
        let bytes = res
            .into_body()
            .collect()
            .await
            .map_err(|e| WolfError(e.to_string()))?
            .to_bytes();
        let value: Value = serde_json::from_slice(&bytes).map_err(|e| WolfError(e.to_string()))?;
        if value["success"] == json!(false) {
            return Err(WolfError(
                value["error"].as_str().unwrap_or("unknown error").into(),
            ));
        }
        Ok(value)
    }

    pub async fn pending_pair_requests(&self) -> Result<Vec<PendingPair>, WolfError> {
        let v = self.request("GET", "/api/v1/pair/pending", None).await?;
        serde_json::from_value(v["requests"].clone()).map_err(|e| WolfError(e.to_string()))
    }

    pub async fn pair(&self, pair_secret: &str, pin: &str) -> Result<String, WolfError> {
        let v = self
            .request(
                "POST",
                "/api/v1/pair/client",
                Some(json!({"pair_secret": pair_secret, "pin": pin})),
            )
            .await?;
        v["client_id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| WolfError("pair response missing client_id".into()))
    }

    pub async fn apps(&self) -> Result<Vec<WolfApp>, WolfError> {
        let v = self.request("GET", "/api/v1/apps", None).await?;
        serde_json::from_value(v["apps"].clone()).map_err(|e| WolfError(e.to_string()))
    }

    pub async fn paired_clients(&self) -> Result<Vec<PairedClient>, WolfError> {
        let v = self.request("GET", "/api/v1/clients", None).await?;
        serde_json::from_value(v["clients"].clone()).map_err(|e| WolfError(e.to_string()))
    }

    pub async fn unpair(&self, client_id: &str) -> Result<(), WolfError> {
        self.request(
            "POST",
            "/api/v1/unpair/client",
            Some(json!({"client_id": client_id})),
        )
        .await
        .map(|_| ())
    }
}
