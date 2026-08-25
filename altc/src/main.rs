use altc_api::{
    db,
    events::{EventHub, spawn_bridge},
    routes,
    state::AppState,
    tls,
    wolf::WolfClient,
};
use std::{fs, path::PathBuf};

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
    db::report_claim_state(&pool).await;

    let (cert, key) = tls::ensure_tls_cert(&state_dir);
    let listeners = altc_api::serve::Listeners::from_env();
    if listeners.http.is_none() && listeners.https.is_none() {
        panic!("both ALTC_API_PORT and ALTC_HTTP_PORT are disabled: nothing could reach the agent");
    }

    let wolf = WolfClient::from_env();
    let events = EventHub::new();
    spawn_bridge(wolf.clone(), events.clone(), pool.clone());

    let library = altc_api::storage::Library::from_env();
    let art_dir = state_dir.join("art");
    fs::create_dir_all(&art_dir).expect("create art cache dir");
    altc_api::routes::warn_if_recovery_armed(&state_dir);
    let app = routes::router(AppState {
        state_dir: state_dir.clone(),
        pool,
        wolf,
        events,
        library,
        art_dir,
    });
    tracing::info!("altc-api listening on {}", listeners.describe());
    altc_api::serve::warn_about_http(&listeners);
    altc_api::serve::run(app, listeners, &cert, &key).await;
}
