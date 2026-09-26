use std::sync::Arc;

use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;

use crate::auth::JwtKeys;
use crate::channels::{ChannelRegistry, SharedPipelinePolicies};
use crate::config::ServerConfig;
use crate::event::EventBus;
use crate::module::ModuleRegistry;
use crate::plugins::PluginStorageService;
use crate::providers::sensor::SensorRegistry;
use crate::registry::ArtifactRegistry;
use crate::scheduler::{ActionRegistry, ConditionRegistry};
use crate::settings::SettingsRegistry;

/// Application global state, passed to Axum handlers via State extractor
#[derive(Clone)]
pub struct AppState {
    pub runtime_tasks: Arc<crate::module::lifecycle::RuntimeTasks>,
    pub execution: Arc<crate::modules::execution::ExecutionService>,
    pub delivery: Arc<crate::modules::delivery::DeliveryService>,
    pub voice_input: crate::modules::interaction::voice_input::VoiceInputHandle,
    pub voice_initiatives: crate::modules::interaction::initiative::VoiceInitiativeHandle,
    pub pipeline_task: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pub turn_detector: Option<Arc<dyn crate::modules::interaction::turn_detector::TurnDetector>>,
    pub db: DatabaseConnection,
    pub channels: Arc<ChannelRegistry>,
    pub event_bus: EventBus,
    pub artifacts: Arc<ArtifactRegistry>,
    pub plugin_storage: Arc<PluginStorageService>,
    pub modules: Arc<ModuleRegistry>,
    pub sensors: Arc<SensorRegistry>,
    pub conditions: Arc<ConditionRegistry>,
    pub actions: Arc<ActionRegistry>,
    pub config: Arc<ServerConfig>,
    pub jwt_keys: Arc<JwtKeys>,
    pub settings: SettingsRegistry,
    pub pipeline_policies: SharedPipelinePolicies,
    pub cancel_token: CancellationToken,
}
