use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::ExecutionService;
use crate::modules::platform::lifecycle::{ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    ToolCall, ToolContext, ToolDefinition, ToolEntry, ToolRegistry, ToolResult,
    ToolRoutingIndexEntry,
};

/// The only registry handed to product modules. Raw executor resolution is
/// intentionally not delegated, while catalog/plugin operations remain shared.
pub struct ExecutionToolRegistry {
    execution: Arc<ExecutionService>,
}

impl ExecutionToolRegistry {
    pub fn new(execution: Arc<ExecutionService>) -> Self {
        Self { execution }
    }
}

#[async_trait]
impl ModuleLifecycle for ExecutionToolRegistry {
    fn name(&self) -> &'static str {
        "execution"
    }
    async fn start(&self) -> anyhow::Result<()> {
        self.execution.raw.start().await?;
        if let Err(error) = self.execution.start().await {
            if let Err(cleanup) = self.execution.raw.stop().await {
                return Err(error.context(format!(
                    "raw tool registry rollback also failed: {cleanup:#}"
                )));
            }
            return Err(error);
        }
        Ok(())
    }
    async fn stop(&self) -> anyhow::Result<()> {
        let execution = self.execution.shutdown().await;
        // A timed-out join retains a live executor. Its adapters remain owned
        // until a later stop can finish that join; they must not stop underneath it.
        if self.execution.worker_pending() {
            return execution;
        }
        let raw = self.execution.raw.stop().await;
        execution.and(raw)
    }
    async fn health_check(&self) -> ModuleHealth {
        self.execution.health()
    }
    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let mut applied = self.execution.apply_settings(updates).await;
        applied.extend(self.execution.raw.apply_settings(updates).await?);
        Ok(applied)
    }
}

#[async_trait]
impl ToolRegistry for ExecutionToolRegistry {
    async fn list_tools(&self) -> anyhow::Result<Vec<ToolDefinition>> {
        self.execution.raw.list_tools().await
    }
    async fn list_tools_for(&self, user_id: uuid::Uuid) -> anyhow::Result<Vec<ToolDefinition>> {
        self.execution.raw.list_tools_for(user_id).await
    }
    async fn get_tool(&self, name: &str) -> anyhow::Result<Option<ToolDefinition>> {
        self.execution.raw.get_tool(name).await
    }
    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        self.execution.execute(call, ctx).await
    }
    async fn register_tool_executor(
        &self,
        owner: &str,
        executor: Arc<ToolEntry>,
    ) -> anyhow::Result<()> {
        let _catalog = self.execution.catalog_gate.write().await;
        self.execution
            .raw
            .register_tool_executor(owner, executor)
            .await
    }
    async fn replace_source_tools(
        &self,
        owner: &str,
        entries: Vec<Arc<ToolEntry>>,
    ) -> anyhow::Result<()> {
        let _catalog = self.execution.catalog_gate.write().await;
        self.execution
            .raw
            .replace_source_tools(owner, entries)
            .await
    }
    async fn unregister_plugin_tool(&self, name: &str) -> anyhow::Result<()> {
        let _catalog = self.execution.catalog_gate.write().await;
        self.execution.raw.unregister_plugin_tool(name).await
    }
    async fn unregister_plugin_tools(&self, owner: &str) -> anyhow::Result<u64> {
        let _catalog = self.execution.catalog_gate.write().await;
        self.execution.raw.unregister_plugin_tools(owner).await
    }
    async fn routing_index(&self, names: &[String]) -> anyhow::Result<Vec<ToolRoutingIndexEntry>> {
        self.execution.raw.routing_index(names).await
    }
    async fn update_routing_embedding(
        &self,
        name: &str,
        model: &str,
        embedding: &[f32],
    ) -> anyhow::Result<()> {
        self.execution
            .raw
            .update_routing_embedding(name, model, embedding)
            .await
    }
}
