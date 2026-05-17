use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::ServerConfig;
use crate::event::EventBus;
use crate::providers::adapter::{ConversationBackend, MemoryBackend, SchedulerBackend};
use crate::registry::ArtifactRegistry;
use crate::scheduler::{ActionRegistry, ConditionRegistry};

/// Health status of a module
#[derive(Debug, Clone)]
pub enum ModuleHealth {
    Healthy,
    Degraded(String),
    Unhealthy(String),
}

/// Shared context provided to modules during initialization (dependency injection)
pub struct ModuleContext {
    pub db: DatabaseConnection,
    pub config: Arc<ServerConfig>,
    pub event_bus: EventBus,
    pub cancel_token: CancellationToken,
    /// Shared artifact graph for registering ownership/dependencies.
    pub artifacts: Arc<ArtifactRegistry>,
    /// Scheduler reference, available for modules initialized after the scheduler.
    pub scheduler: Option<Arc<dyn SchedulerBackend>>,
    /// Condition registry for registering/evaluating task conditions.
    pub conditions: Option<Arc<ConditionRegistry>>,
    /// Action registry shared by scheduled tasks, event rules, and plugins.
    pub actions: Option<Arc<ActionRegistry>>,
    /// Conversation backend for managing sessions (available to channels).
    pub conversation: Option<Arc<dyn ConversationBackend>>,
    /// Memory backend for tools/components initialized after memory.
    pub memory: Option<Arc<dyn MemoryBackend>>,
}

/// Base trait for all pluggable modules
#[async_trait]
pub trait ModuleLifecycle: Send + Sync {
    /// Module identifier (used in logs and config references)
    fn name(&self) -> &'static str;

    /// Initialize: receive shared context, establish connections, load resources
    async fn init(&mut self, _ctx: &ModuleContext) -> anyhow::Result<()> {
        Ok(())
    }

    /// Start: called after all modules are initialized, can start background tasks
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Stop: called in reverse order during graceful shutdown
    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Health check for /health endpoint aggregation
    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }

    /// Apply runtime setting changes that do not require rebuilding the module.
    ///
    /// Modules should return the dotted setting keys they consumed. Unknown keys
    /// are ignored so plugins and future modules can share one update path.
    async fn apply_settings(
        &self,
        _updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }
}
