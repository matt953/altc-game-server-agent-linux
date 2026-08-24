use crate::events::EventHub;
use crate::wolf::WolfClient;
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct AppState {
    /// Where fetched box art is cached; served back and read by Wolf.
    pub art_dir: std::path::PathBuf,
    pub library: crate::storage::Library,
    pub pool: SqlitePool,
    pub wolf: WolfClient,
    pub events: EventHub,
}
