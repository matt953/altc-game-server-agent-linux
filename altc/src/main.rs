use altc_api::{
    db,
    events::{EventHub, spawn_bridge},
    routes,
    state::AppState,
    tls,
    wolf::WolfClient,
};
use std::{fs, net::SocketAddr, path::PathBuf};

#[tokio::main]
async fn main() {
    // ALTC_LOG, not RUST_LOG: the agent image sets RUST_LOG=WARN for the
    // Wayland compositor, which would silence us.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
                .with_env_var("ALTC_LOG")
                .from_env_lossy(),
        )
        .init();

    let state_dir = PathBuf::from(
        std::env::var("HOST_APPS_STATE_FOLDER").unwrap_or_else(|_| "/etc/wolf".into()),
    )
    .join("altc");
    fs::create_dir_all(&state_dir).expect("create altc state dir");

    let pool = db::init(&state_dir).await;
    db::bootstrap_owner(&pool).await;

    let (cert, key) = tls::ensure_tls_cert(&state_dir);
    let port: u16 = std::env::var("ALTC_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(47990);

    let wolf = WolfClient::from_env();
    let events = EventHub::new();
    spawn_bridge(wolf.clone(), events.clone(), pool.clone());

    let library = altc_api::storage::Library::from_env();
    let app = routes::router(AppState {
        pool,
        wolf,
        events,
        library,
    });
    let tls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
        .await
        .expect("load tls cert");
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("altc-api listening on https://{addr}");
    axum_server::bind_rustls(addr, tls_config)
        .serve(app.into_make_service())
        .await
        .expect("server");
}
