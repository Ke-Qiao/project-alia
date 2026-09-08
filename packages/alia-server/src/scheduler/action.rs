//! Shared action execution for scheduled tasks and event rules.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::event::{EventBus, ServerEvent};
use crate::providers::adapter::{
    ActionContext, ActionExecutor, ToolCall, ToolContext, ToolRegistry,
};

/// Runtime registry for action executors.
///
/// Plugins and components register custom action executors here. Rules and
/// scheduled tasks both execute through this registry so action semantics stay
/// consistent.
pub struct ActionRegistry {
    executors: Arc<RwLock<HashMap<String, Arc<dyn ActionExecutor>>>>,
}

impl ActionRegistry {
    pub fn new() -> Self {
        Self {
            executors: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register(&self, executor: Arc<dyn ActionExecutor>) {
        let action_type = executor.action_type().to_string();
        tracing::info!(action_type = %action_type, "registered action executor");
        self.executors.write().await.insert(action_type, executor);
    }

    pub async fn execute(
        &self,
        action: &serde_json::Value,
        ctx: &ActionContext,
    ) -> anyhow::Result<serde_json::Value> {
        let action_type = action
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("action.type is required"))?;

        let executors = self.executors.read().await;
        let executor = executors
            .get(action_type)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown action type: {action_type}"))?;
        drop(executors);

        executor.execute(action, ctx).await
    }

    pub async fn registered_types(&self) -> Vec<String> {
        self.executors.read().await.keys().cloned().collect()
    }
}

impl Default for ActionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Capture once per owner before condition evaluation or action queueing. An
/// unowned system action keeps None; it cannot acquire memory authority later.
pub(crate) async fn capture_action_sources(
    db: &sea_orm::DatabaseConnection,
    owners: impl IntoIterator<Item = uuid::Uuid>,
) -> HashMap<uuid::Uuid, Option<i64>> {
    let mut epochs = HashMap::new();
    let owners = owners.into_iter().collect::<std::collections::HashSet<_>>();
    for owner in owners {
        if owner.is_nil() {
            epochs.insert(owner, None);
            continue;
        }
        match crate::modules::memory::source::capture_source_epoch(db, owner).await {
            Ok(epoch) => {
                epochs.insert(owner, Some(epoch));
            }
            Err(error) => {
                tracing::warn!(%owner, error = %error, "automation source admission rejected; action not queued");
            }
        }
    }
    epochs
}

pub struct LogActionExecutor;

#[async_trait]
impl ActionExecutor for LogActionExecutor {
    fn action_type(&self) -> &str {
        "log"
    }

    async fn execute(
        &self,
        config: &serde_json::Value,
        ctx: &ActionContext,
    ) -> anyhow::Result<serde_json::Value> {
        let message = config
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("automation action triggered");
        tracing::info!(
            source = %ctx.source,
            trigger_id = ?ctx.trigger_id,
            trigger_name = ?ctx.trigger_name,
            message = %message,
            "automation log action"
        );
        Ok(serde_json::json!({ "logged": message }))
    }
}

pub struct NoopActionExecutor {
    action_type: &'static str,
}

impl NoopActionExecutor {
    pub fn new(action_type: &'static str) -> Self {
        Self { action_type }
    }
}

#[async_trait]
impl ActionExecutor for NoopActionExecutor {
    fn action_type(&self) -> &str {
        self.action_type
    }

    async fn execute(
        &self,
        config: &serde_json::Value,
        ctx: &ActionContext,
    ) -> anyhow::Result<serde_json::Value> {
        tracing::debug!(
            action_type = self.action_type,
            source = %ctx.source,
            trigger_id = ?ctx.trigger_id,
            config = ?config,
            "automation no-op action"
        );
        Ok(serde_json::json!({ "noop": true, "type": self.action_type }))
    }
}

/// Emits a `ServerEvent::PluginEvent` for loose coupling with plugins/components.
pub struct EmitEventActionExecutor {
    event_bus: EventBus,
}

impl EmitEventActionExecutor {
    pub fn new(event_bus: EventBus) -> Self {
        Self { event_bus }
    }
}

#[async_trait]
impl ActionExecutor for EmitEventActionExecutor {
    fn action_type(&self) -> &str {
        "emit_event"
    }

    async fn execute(
        &self,
        config: &serde_json::Value,
        ctx: &ActionContext,
    ) -> anyhow::Result<serde_json::Value> {
        let plugin_name = config
            .get("plugin_name")
            .and_then(|v| v.as_str())
            .unwrap_or("automation")
            .to_string();
        let event_type = config
            .get("event_type")
            .and_then(|v| v.as_str())
            .unwrap_or("action")
            .to_string();
        let data = config.get("data").cloned().unwrap_or_else(|| {
            serde_json::json!({
                "source": ctx.source,
                "trigger_id": ctx.trigger_id,
                "trigger_name": ctx.trigger_name,
            })
        });

        self.event_bus.publish(ServerEvent::PluginEvent {
            plugin_name,
            event_type,
            data,
        });

        Ok(serde_json::json!({ "emitted": true }))
    }
}

pub struct ToolCallActionExecutor {
    tools: Arc<dyn ToolRegistry>,
}

impl ToolCallActionExecutor {
    pub fn new(tools: Arc<dyn ToolRegistry>) -> Self {
        Self { tools }
    }
}

#[async_trait]
impl ActionExecutor for ToolCallActionExecutor {
    fn action_type(&self) -> &str {
        "tool_call"
    }

    async fn execute(
        &self,
        config: &serde_json::Value,
        ctx: &ActionContext,
    ) -> anyhow::Result<serde_json::Value> {
        let tool_name = config
            .get("tool_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("tool_call.tool_name is required"))?
            .to_string();
        let arguments = config
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let call_id = config
            .get("call_id")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("{}:0", ctx.request_id));

        let result = self
            .tools
            .execute(
                ToolCall {
                    tool_name,
                    arguments,
                    call_id,
                },
                &ToolContext {
                    user_id: ctx.user_id,
                    request_id: ctx.request_id,
                    source_received_at: ctx.source_received_at,
                    source_event_id: None,
                    source_privacy_epoch: ctx.source_privacy_epoch,
                    invocation_key: "0".into(),
                    conversation_id: None,
                    source: ctx.source.clone(),
                    channel_id: None,
                    cancel: None,
                },
            )
            .await?;

        Ok(serde_json::to_value(result)?)
    }
}
