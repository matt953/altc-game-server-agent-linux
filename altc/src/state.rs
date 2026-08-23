use crate::events::EventHub;
use crate::wolf::WolfClient;
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub wolf: WolfClient,
    pub events: EventHub,
}
