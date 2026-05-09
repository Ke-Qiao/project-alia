use std::sync::Arc;

use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;

use crate::auth::JwtKeys;
use crate::channels::ChannelRegistry;
use crate::config::ServerConfig;
use crate::event::EventBus;
use crate::module::ModuleRegistry;
use crate::providers::sensor::SensorRegistry;
use crate::scheduler::ConditionRegistry;

/// Application global state, passed to Axum handlers via State extractor
#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub channels: Arc<ChannelRegistry>,
    pub event_bus: EventBus,
    pub modules: Arc<ModuleRegistry>,
    pub sensors: Arc<SensorRegistry>,
    pub conditions: Arc<ConditionRegistry>,
    pub config: Arc<ServerConfig>,
    pub jwt_keys: Arc<JwtKeys>,
    pub cancel_token: CancellationToken,
}
