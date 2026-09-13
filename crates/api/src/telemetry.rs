use serde::Serialize;
use space_station::SpaceClient;
use std::sync::Arc;

/// Best-effort writers for the four Starter Space Station tables.
/// Missing keys disable telemetry; ingestion failures never fail requests.
pub struct Telemetry {
    backend: Option<Arc<SpaceClient>>,
    cli: Option<Arc<SpaceClient>>,
    frontend_analytics: Option<Arc<SpaceClient>>,
    frontend_events: Option<Arc<SpaceClient>>,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::from_env()
    }
}

impl Telemetry {
    pub fn from_env() -> Self {
        Self {
            backend: client("STARTER_BACKEND_TABLE_KEY"),
            cli: client("STARTER_CLI_TABLE_KEY"),
            frontend_analytics: client("STARTER_FRONTEND_ANALYTICS_TABLE_KEY"),
            frontend_events: client("STARTER_FRONTEND_EVENTS_TABLE_KEY"),
        }
    }
    pub fn record<T: Serialize>(&self, table: &str, event: T) {
        let c = match table {
            "backend" => &self.backend,
            "cli" => &self.cli,
            "frontend_analytics" => &self.frontend_analytics,
            "frontend_events" => &self.frontend_events,
            _ => return,
        };
        if let Some(c) = c {
            c.record(event);
        }
    }
}

fn client(name: &str) -> Option<Arc<SpaceClient>> {
    if std::env::var("SPACE_STATION_TELEMETRY").ok().as_deref() == Some("0") {
        return None;
    }
    std::env::var(name)
        .ok()
        .and_then(|key| SpaceClient::new(&key).ok())
        .map(Arc::new)
}
