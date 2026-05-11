//! Tools module: extend what the LLM **can do**.
//!
//! Tools are executable functions the LLM (or skills) can call via function
//! calling. They provide the actual actions — reading sensors, controlling
//! devices, managing memory, etc.
//!
//! This module provides:
//! - `NoopToolRegistry`: A no-op implementation for disabled tools
//! - `BuiltinToolRegistry`: Full implementation with built-in tools and plugin support
//! - Sensor tools: `sensor_read`, `sensor_list`

pub mod builtin;
pub mod sensor;

pub use builtin::BuiltinToolRegistry;

use async_trait::async_trait;

use crate::module::ModuleLifecycle;
use crate::providers::adapter::{ToolCall, ToolContext, ToolDefinition, ToolRegistry, ToolResult};

/// No-op tool registry implementation for disabled state.
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

    async fn execute(&self, call: ToolCall, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        Ok(ToolResult {
            call_id: call.call_id,
            success: false,
            output: serde_json::json!({"error": "tools are disabled"}),
            display: Some("Tools are disabled".to_string()),
        })
    }

    async fn register_plugin_tool(&self, _tool: ToolDefinition) -> anyhow::Result<()> {
        anyhow::bail!("tools are disabled")
    }

    async fn unregister_plugin_tool(&self, _tool_name: &str) -> anyhow::Result<()> {
        anyhow::bail!("tools are disabled")
    }
}
