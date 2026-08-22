use axum::{routing::get, Json, Router};
use std::{fs, net::SocketAddr, path::{Path, PathBuf}};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();

    let state_dir = PathBuf::from(
        std::env::var("HOST_APPS_STATE_FOLDER").unwrap_or_else(|_| "/etc/wolf".into()),
    )
    .join("altc");
    fs::create_dir_all(&state_dir).expect("create altc state dir");

    let (cert, key) = ensure_tls_cert(&state_dir);
    let port: u16 = std::env::var("ALTC_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(47990);

    let app = Router::new().route("/healthz", get(healthz));
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
        .await
        .expect("load tls cert");

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("altc-api listening on https://{addr}");
    axum_server::bind_rustls(addr, tls)
        .serve(app.into_make_service())
        .await
        .expect("server");
}

async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "altc-api",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

// Self-signed cert generated once, persisted so clients can trust-on-first-use.
fn ensure_tls_cert(dir: &Path) -> (PathBuf, PathBuf) {
    let cert_path = dir.join("api-cert.pem");
    let key_path = dir.join("api-key.pem");
    if !cert_path.exists() || !key_path.exists() {
        let ck = rcgen::generate_simple_self_signed(vec!["altc".into()])
            .expect("generate self-signed cert");
        fs::write(&cert_path, ck.cert.pem()).expect("write cert");
        fs::write(&key_path, ck.key_pair.serialize_pem()).expect("write key");
    }
    (cert_path, key_path)
}
