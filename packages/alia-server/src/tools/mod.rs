//! Tool Registry module: AI function calling and tool management
//!
//! Provides a registry of tools that LLM can call via function_calling / tool_use.
//! Supports both built-in tools and dynamically-registered plugin tools.
//!
//! Tools include:
//! - Scheduler operations (create_task, list_tasks, delete_task, subscribe_event)
//! - Device control (get_status, send_command)
//! - Memory operations (search, store)
//! - Notifications
//! - Plugin commands
//! - TTS announcements

use async_trait::async_trait;

use crate::module::ModuleLifecycle;
use crate::providers::adapter::{ToolDefinition, ToolCall, ToolResult, ToolContext, ToolRegistry};

mod builtin;
pub use builtin::BuiltinToolRegistry;

/// No-op tool registry: no tools available
pub struct NoopToolRegistry;

#[async_trait]
impl ModuleLifecycle for NoopToolRegistry {
    fn name(&self) -> &'static str {
        "noop_tools"
    }
}

#[async_trait]
impl ToolRegistry for NoopToolRegistry {
    async fn list_tools(&self) -> anyhow::Result<Vec<ToolDefinition>> {
        Ok(Vec::new())
    }

    async fn get_tool(&self, _name: &str) -> anyhow::Result<Option<ToolDefinition>> {
        Ok(None)
    }

    async fn execute(&self, _call: ToolCall, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        Err(anyhow::anyhow!("No tools available in noop registry"))
    }

    async fn register_plugin_tool(&self, _tool: ToolDefinition) -> anyhow::Result<()> {
        Ok(())
    }

    async fn unregister_plugin_tool(&self, _tool_name: &str) -> anyhow::Result<()> {
        Ok(())
    }
}
