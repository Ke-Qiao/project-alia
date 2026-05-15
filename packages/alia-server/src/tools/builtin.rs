//! BuiltinToolRegistry: Full tool registry with built-in tools and plugin support.
//!
//! ## Built-in Tools
//! - `get_current_time`: Get current date/time in specified timezone
//! - `get_weather`: Get weather for a location (stub, requires plugin)
//! - `memory_search`: Search user's memory graph
//! - `memory_store`: Store information to memory
//! - `device_control`: Control connected devices
//! - `schedule_task`: Create a scheduled task
//! - `send_notification`: Send notification to user
//!
//! ## Plugin Tools
//! Plugins can register custom tools at runtime.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::entity::tool;
use crate::event::{EventBus, ServerEvent};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{ToolCall, ToolContext, ToolDefinition, ToolRegistry, ToolResult};
use crate::providers::sensor::SensorRegistry;
use crate::tools::sensor::{SensorListTool, SensorReadTool};

/// Trait for implementing tool executors
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn execute(
        &self,
        arguments: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult>;
}

/// BuiltinToolRegistry: Full implementation of ToolRegistry
pub struct BuiltinToolRegistry {
    /// Built-in tools (immutable after init)
    builtins: HashMap<String, Arc<dyn ToolExecutor>>,
    /// Plugin-registered tools (in-memory cache, backed by DB)
    plugins: Arc<RwLock<HashMap<String, ToolDefinition>>>,
    /// Event bus for publishing tool events
    event_bus: tokio::sync::Mutex<Option<EventBus>>,
    /// DB connection for persisting plugin tools
    db: Option<DatabaseConnection>,
}

impl BuiltinToolRegistry {
    pub fn new() -> Self {
        Self {
            builtins: HashMap::new(),
            plugins: Arc::new(RwLock::new(HashMap::new())),
            event_bus: tokio::sync::Mutex::new(None),
            db: None,
        }
    }

    /// Create with sensor registry integration (sensor_read + sensor_list tools).
    pub fn with_sensors(sensors: Arc<SensorRegistry>) -> Self {
        let mut this = Self::new();
        this.builtins.insert(
            "sensor_read".to_string(),
            Arc::new(SensorReadTool::new(sensors.clone())),
        );
        this.builtins.insert(
            "sensor_list".to_string(),
            Arc::new(SensorListTool::new(sensors)),
        );
        this
    }

    fn register_builtins(&mut self) {
        // get_current_time
        self.builtins
            .insert("get_current_time".to_string(), Arc::new(GetCurrentTime));

        // memory_search
        self.builtins
            .insert("memory_search".to_string(), Arc::new(MemorySearch));

        // memory_store
        self.builtins
            .insert("memory_store".to_string(), Arc::new(MemoryStore));

        // schedule_task
        self.builtins
            .insert("schedule_task".to_string(), Arc::new(ScheduleTask));

        // send_notification
        self.builtins
            .insert("send_notification".to_string(), Arc::new(SendNotification));

        // device_control
        self.builtins
            .insert("device_control".to_string(), Arc::new(DeviceControl));
    }

    /// Register an additional tool executor at runtime (for tools that need external state).
    pub fn register_executor(&mut self, executor: Arc<dyn ToolExecutor>) {
        let name = executor.definition().name;
        self.builtins.insert(name, executor);
    }

    /// Load plugin tools from DB into in-memory cache.
    async fn load_plugin_tools_from_db(&self) -> anyhow::Result<()> {
        let Some(db) = &self.db else { return Ok(()) };

        let rows = tool::Entity::find()
            .filter(tool::Column::Enabled.eq(true))
            .all(db)
            .await?;

        let mut plugins = self.plugins.write().unwrap();
        for row in rows {
            plugins.insert(
                row.name.clone(),
                ToolDefinition {
                    name: row.name,
                    description: row.description,
                    parameters: row.parameters,
                    source: row.source,
                    require_confirmation: row.require_confirmation,
                    required_role: row.required_role,
                },
            );
        }

        Ok(())
    }
}

impl Default for BuiltinToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for BuiltinToolRegistry {
    fn name(&self) -> &'static str {
        "builtin_tools"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        *self.event_bus.lock().await = Some(ctx.event_bus.clone());
        self.db = Some(ctx.db.clone());
        self.register_builtins();

        // Load plugin-registered tools from DB
        if let Err(e) = self.load_plugin_tools_from_db().await {
            tracing::warn!(error = %e, "failed to load plugin tools from DB");
        }

        let plugin_count = self.plugins.read().unwrap().len();
        tracing::info!(
            builtin_count = self.builtins.len(),
            plugin_count,
            "BuiltinToolRegistry initialized"
        );
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }
}

#[async_trait]
impl ToolRegistry for BuiltinToolRegistry {
    async fn list_tools(&self) -> anyhow::Result<Vec<ToolDefinition>> {
        let mut tools: Vec<ToolDefinition> = self
            .builtins
            .values()
            .map(|executor| executor.definition())
            .collect();

        // Add plugin tools
        let plugins = self.plugins.read().unwrap();
        tools.extend(plugins.values().cloned());

        Ok(tools)
    }

    async fn get_tool(&self, name: &str) -> anyhow::Result<Option<ToolDefinition>> {
        // Check builtins first
        if let Some(executor) = self.builtins.get(name) {
            return Ok(Some(executor.definition()));
        }

        // Check plugins
        let plugins = self.plugins.read().unwrap();
        Ok(plugins.get(name).cloned())
    }

    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let call_id = call.call_id.clone();
        let tool_name = call.tool_name.clone();

        // Publish start event
        if let Some(event_bus) = self.event_bus.lock().await.as_ref() {
            event_bus.publish(ServerEvent::ToolCallStarted {
                call_id: call_id.clone(),
                tool_name: tool_name.clone(),
                source: ctx.source.clone(),
            });
        }

        let start = std::time::Instant::now();

        // Execute
        let result = if let Some(executor) = self.builtins.get(&call.tool_name) {
            executor.execute(call.arguments, ctx).await
        } else {
            // Check if it's a plugin tool
            let is_plugin = {
                let plugins = self.plugins.read().unwrap();
                plugins.contains_key(&call.tool_name)
            };

            if is_plugin {
                // TODO: Plugin tool execution via gRPC to alia-embedded
                Ok(ToolResult {
                    call_id: call_id.clone(),
                    success: false,
                    output: serde_json::json!({"error": "plugin tool execution not implemented"}),
                    display: Some("Plugin tool execution not yet implemented".to_string()),
                })
            } else {
                Ok(ToolResult {
                    call_id: call_id.clone(),
                    success: false,
                    output: serde_json::json!({"error": format!("unknown tool: {}", call.tool_name)}),
                    display: Some(format!("Unknown tool: {}", call.tool_name)),
                })
            }
        };

        let duration_ms = start.elapsed().as_millis() as u64;

        // Publish completion event
        if let Some(event_bus) = self.event_bus.lock().await.as_ref() {
            event_bus.publish(ServerEvent::ToolCallCompleted {
                call_id: call_id.clone(),
                tool_name,
                success: result.as_ref().map_or(false, |r| r.success),
                duration_ms,
            });
        }

        result.map(|mut result| {
            if result.call_id.is_empty() {
                result.call_id = call_id;
            }
            result
        })
    }

    async fn register_plugin_tool(&self, tool: ToolDefinition) -> anyhow::Result<()> {
        if self.builtins.contains_key(&tool.name) {
            anyhow::bail!("cannot override builtin tool: {}", tool.name);
        }

        // Persist to DB
        if let Some(db) = &self.db {
            let now = Utc::now();
            let existing = tool::Entity::find()
                .filter(tool::Column::Name.eq(&tool.name))
                .one(db)
                .await?;

            match existing {
                Some(row) => {
                    let mut active: tool::ActiveModel = row.into();
                    active.description = Set(tool.description.clone());
                    active.parameters = Set(tool.parameters.clone());
                    active.source = Set(tool.source.clone());
                    active.require_confirmation = Set(tool.require_confirmation);
                    active.required_role = Set(tool.required_role.clone());
                    active.enabled = Set(true);
                    active.updated_at = Set(now);
                    active.update(db).await?;
                }
                None => {
                    let active = tool::ActiveModel {
                        id: Set(Uuid::new_v4()),
                        name: Set(tool.name.clone()),
                        description: Set(tool.description.clone()),
                        parameters: Set(tool.parameters.clone()),
                        source: Set(tool.source.clone()),
                        require_confirmation: Set(tool.require_confirmation),
                        required_role: Set(tool.required_role.clone()),
                        enabled: Set(true),
                        created_at: Set(now),
                        updated_at: Set(now),
                    };
                    active.insert(db).await?;
                }
            }
        }

        // Update in-memory cache
        let mut plugins = self.plugins.write().unwrap();
        tracing::info!(tool = %tool.name, source = %tool.source, "registered plugin tool");
        plugins.insert(tool.name.clone(), tool);
        Ok(())
    }

    async fn unregister_plugin_tool(&self, tool_name: &str) -> anyhow::Result<()> {
        // Remove from DB
        if let Some(db) = &self.db {
            tool::Entity::delete_many()
                .filter(tool::Column::Name.eq(tool_name))
                .exec(db)
                .await?;
        }

        // Remove from in-memory cache
        let mut plugins = self.plugins.write().unwrap();
        plugins
            .remove(tool_name)
            .ok_or_else(|| anyhow::anyhow!("plugin tool not found: {tool_name}"))?;
        tracing::info!(tool = %tool_name, "unregistered plugin tool");
        Ok(())
    }
}

// ============ Built-in Tool Implementations ============

/// Get current date/time
pub struct GetCurrentTime;

#[async_trait]
impl ToolExecutor for GetCurrentTime {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "get_current_time".to_string(),
            description: "Get the current date and time. Optionally specify a timezone."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "timezone": {
                        "type": "string",
                        "description": "IANA timezone name (e.g., 'America/New_York', 'Asia/Tokyo'). Defaults to UTC."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["iso8601", "human", "unix"],
                        "description": "Output format. Defaults to 'human'."
                    }
                }
            }),
            source: "builtin".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let format = arguments
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("human");

        let now = Utc::now();
        // TODO: Handle timezone conversion with chrono-tz

        let time_str = match format {
            "iso8601" => now.to_rfc3339(),
            "unix" => now.timestamp().to_string(),
            _ => now.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        };

        Ok(ToolResult {
            call_id: String::new(), // Will be filled by caller
            success: true,
            output: serde_json::json!({
                "datetime": time_str,
                "timezone": "UTC",
                "timestamp": now.timestamp()
            }),
            display: Some(time_str),
        })
    }
}

/// Search user's memory
struct MemorySearch;

#[async_trait]
impl ToolExecutor for MemorySearch {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "memory_search".to_string(),
            description: "Search the user's memory for relevant information.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query to find relevant memories"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return. Defaults to 5."
                    },
                    "layers": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Memory layers to search: 'core', 'episodic', 'semantic'. Defaults to all."
                    }
                },
                "required": ["query"]
            }),
            source: "builtin".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("query is required"))?;

        // TODO: Integrate with MemoryBackend
        tracing::info!(%query, "memory_search called (stub)");

        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "results": [],
                "message": "Memory search not yet integrated with MemoryBackend"
            }),
            display: Some("No memories found (integration pending)".to_string()),
        })
    }
}

/// Store information to memory
struct MemoryStore;

#[async_trait]
impl ToolExecutor for MemoryStore {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "memory_store".to_string(),
            description: "Store a piece of information to the user's memory.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "The information to remember"
                    },
                    "layer": {
                        "type": "string",
                        "enum": ["core", "episodic", "semantic"],
                        "description": "Memory layer to store in. Defaults to 'episodic'."
                    },
                    "importance": {
                        "type": "number",
                        "description": "Importance score from 0.0 to 1.0. Defaults to 0.5."
                    }
                },
                "required": ["content"]
            }),
            source: "builtin".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("content is required"))?;

        // TODO: Integrate with MemoryBackend
        tracing::info!(content_len = content.len(), "memory_store called (stub)");

        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "stored": false,
                "message": "Memory store not yet integrated with MemoryBackend"
            }),
            display: Some("Memory storage pending integration".to_string()),
        })
    }
}

/// Schedule a task
struct ScheduleTask;

#[async_trait]
impl ToolExecutor for ScheduleTask {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "schedule_task".to_string(),
            description:
                "Create a scheduled task to run at a specific time or on a recurring schedule."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the scheduled task"
                    },
                    "schedule_type": {
                        "type": "string",
                        "enum": ["cron", "once", "interval"],
                        "description": "Type of schedule"
                    },
                    "cron_expression": {
                        "type": "string",
                        "description": "Cron expression for recurring tasks (e.g., '0 9 * * *' for daily at 9am)"
                    },
                    "run_at": {
                        "type": "string",
                        "description": "ISO 8601 datetime for one-time tasks"
                    },
                    "every_sec": {
                        "type": "integer",
                        "description": "Interval in seconds for recurring tasks"
                    },
                    "action": {
                        "type": "object",
                        "description": "Action to execute when triggered"
                    }
                },
                "required": ["name", "schedule_type", "action"]
            }),
            source: "builtin".to_string(),
            require_confirmation: true,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("name is required"))?;

        // TODO: Integrate with SchedulerBackend
        tracing::info!(%name, "schedule_task called (stub)");

        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "scheduled": false,
                "message": "Scheduler integration pending"
            }),
            display: Some(format!("Task '{}' scheduling pending integration", name)),
        })
    }
}

/// Send notification
struct SendNotification;

#[async_trait]
impl ToolExecutor for SendNotification {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "send_notification".to_string(),
            description: "Send a notification to the user.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Notification title"
                    },
                    "body": {
                        "type": "string",
                        "description": "Notification body text"
                    },
                    "priority": {
                        "type": "string",
                        "enum": ["low", "normal", "high"],
                        "description": "Notification priority. Defaults to 'normal'."
                    }
                },
                "required": ["title", "body"]
            }),
            source: "builtin".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let title = arguments
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("title is required"))?;
        let body = arguments
            .get("body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("body is required"))?;

        // TODO: Send via push notification service or WebSocket
        tracing::info!(%title, body_len = body.len(), "send_notification called (stub)");

        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "sent": false,
                "message": "Notification service pending integration"
            }),
            display: Some(format!("Notification '{}' pending", title)),
        })
    }
}

/// Control connected devices
struct DeviceControl;

#[async_trait]
impl ToolExecutor for DeviceControl {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "device_control".to_string(),
            description: "Control a connected smart home device.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "device_id": {
                        "type": "string",
                        "description": "ID of the device to control"
                    },
                    "action": {
                        "type": "string",
                        "description": "Action to perform (e.g., 'turn_on', 'turn_off', 'set_brightness')"
                    },
                    "params": {
                        "type": "object",
                        "description": "Action parameters (e.g., brightness level)"
                    }
                },
                "required": ["device_id", "action"]
            }),
            source: "builtin".to_string(),
            require_confirmation: true,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let device_id = arguments
            .get("device_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("device_id is required"))?;
        let action = arguments
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("action is required"))?;

        // TODO: Send command via gRPC to alia-embedded
        tracing::info!(%device_id, %action, "device_control called (stub)");

        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "executed": false,
                "message": "Device control pending integration with alia-embedded"
            }),
            display: Some(format!("Device {} action '{}' pending", device_id, action)),
        })
    }
}

/// Generate OpenAI-compatible function definitions for LLM
pub fn to_openai_functions(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters
                }
            })
        })
        .collect()
}

/// Parse OpenAI function call response into ToolCall
pub fn parse_openai_tool_call(function_call: &serde_json::Value) -> anyhow::Result<ToolCall> {
    let name = function_call
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing function name"))?;

    let arguments_str = function_call
        .get("arguments")
        .and_then(|v| v.as_str())
        .unwrap_or("{}");

    let arguments: serde_json::Value = serde_json::from_str(arguments_str)
        .map_err(|e| anyhow::anyhow!("invalid function arguments JSON: {e}"))?;

    let call_id = function_call
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(ToolCall {
        tool_name: name.to_string(),
        arguments,
        call_id,
    })
}
