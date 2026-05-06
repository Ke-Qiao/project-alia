use std::sync::Arc;

use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;

use crate::auth::JwtKeys;
use crate::config::ServerConfig;
use crate::embedded::EmbeddedClient;
use crate::event::EventBus;
use crate::module::ModuleRegistry;

/// Application global state, passed to Axum handlers via State extractor
#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub embedded: EmbeddedClient,
    pub event_bus: EventBus,
    pub modules: Arc<ModuleRegistry>,
    pub config: Arc<ServerConfig>,
    pub jwt_keys: Arc<JwtKeys>,
    pub cancel_token: CancellationToken,
}
