//! BuiltinToolRegistry: Full tool registry with built-in tools and plugin support.
//!
//! ## Built-in Tools
//! - `get_current_time`: Get current date/time in specified timezone
//! - `fetch`: Fetch a public HTTP(S) URL and return bounded text content
//! - `calculate`: Evaluate a basic arithmetic expression
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
#[cfg(feature = "tools-http")]
use std::{net::IpAddr, time::Duration};

use async_trait::async_trait;
use chrono::Utc;
#[cfg(feature = "tools-http")]
use futures::StreamExt;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
#[cfg(feature = "tools-http")]
use tokio::net::lookup_host;
use uuid::Uuid;

use crate::entity::tool;
use crate::event::{EventBus, ServerEvent};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{ToolCall, ToolContext, ToolDefinition, ToolRegistry, ToolResult};
use crate::providers::sensor::SensorRegistry;
use crate::tools::sensor::{SensorListTool, SensorReadTool};

const FETCH_DEFAULT_MAX_BYTES: usize = 64 * 1024;
const FETCH_HARD_MAX_BYTES: usize = 1024 * 1024;
const FETCH_DEFAULT_TIMEOUT_SEC: u64 = 10;
const FETCH_HARD_TIMEOUT_SEC: u64 = 30;

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

        // fetch
        #[cfg(feature = "tools-http")]
        self.builtins
            .insert("fetch".to_string(), Arc::new(FetchUrl));

        // calculate
        self.builtins
            .insert("calculate".to_string(), Arc::new(Calculate));

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

/// Fetch a public HTTP(S) URL.
#[cfg(feature = "tools-http")]
pub struct FetchUrl;

#[cfg(feature = "tools-http")]
#[async_trait]
impl ToolExecutor for FetchUrl {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "fetch".to_string(),
            description: "Fetch a public HTTP(S) URL and return bounded response text. Local/private network addresses are blocked.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Public http:// or https:// URL to fetch"
                    },
                    "max_bytes": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": FETCH_HARD_MAX_BYTES,
                        "description": "Maximum response bytes to read. Defaults to 65536 and is capped at 1048576."
                    },
                    "timeout_sec": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": FETCH_HARD_TIMEOUT_SEC,
                        "description": "Request timeout in seconds. Defaults to 10 and is capped at 30."
                    },
                    "headers": {
                        "type": "object",
                        "additionalProperties": {"type": "string"},
                        "description": "Optional request headers. Do not include secrets."
                    }
                },
                "required": ["url"]
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
        let raw_url = required_str(&arguments, "url")?;
        let url = reqwest::Url::parse(raw_url)
            .map_err(|error| anyhow::anyhow!("invalid url: {error}"))?;
        validate_fetch_url(&url).await?;

        let max_bytes = arguments
            .get("max_bytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(FETCH_DEFAULT_MAX_BYTES as u64)
            .clamp(1, FETCH_HARD_MAX_BYTES as u64) as usize;
        let timeout_sec = arguments
            .get("timeout_sec")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(FETCH_DEFAULT_TIMEOUT_SEC)
            .clamp(1, FETCH_HARD_TIMEOUT_SEC);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_sec))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("alia-server builtin fetch/0.1")
            .build()?;
        let mut request = client.get(url.clone());

        if let Some(headers) = arguments
            .get("headers")
            .and_then(serde_json::Value::as_object)
        {
            for (name, value) in headers {
                let Some(value) = value.as_str() else {
                    anyhow::bail!("header '{name}' must be a string");
                };
                request = request.header(name, value);
            }
        }

        let response = request.send().await?;
        let status = response.status();
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let mut bytes = Vec::with_capacity(max_bytes.min(8192));
        let mut stream = response.bytes_stream();
        let mut truncated = false;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let remaining = max_bytes.saturating_sub(bytes.len());
            if chunk.len() > remaining {
                bytes.extend_from_slice(&chunk[..remaining]);
                truncated = true;
                break;
            }
            bytes.extend_from_slice(&chunk);
            if bytes.len() >= max_bytes {
                truncated = true;
                break;
            }
        }

        let text = String::from_utf8_lossy(&bytes).to_string();
        let display = if status.is_success() {
            format!("Fetched {final_url} ({status}, {} bytes)", bytes.len())
        } else {
            format!("Fetched {final_url} with HTTP status {status}")
        };

        Ok(ToolResult {
            call_id: String::new(),
            success: status.is_success(),
            output: serde_json::json!({
                "url": raw_url,
                "final_url": final_url,
                "status": status.as_u16(),
                "content_type": content_type,
                "text": text,
                "bytes_read": bytes.len(),
                "truncated": truncated,
            }),
            display: Some(display),
        })
    }
}

/// Evaluate a basic arithmetic expression.
pub struct Calculate;

#[async_trait]
impl ToolExecutor for Calculate {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "calculate".to_string(),
            description:
                "Evaluate a basic arithmetic expression using +, -, *, /, ^, and parentheses."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "Arithmetic expression to evaluate, for example '(2 + 3) * 4'."
                    }
                },
                "required": ["expression"]
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
        let expression = required_str(&arguments, "expression")?;
        let result = eval_arithmetic(expression)?;
        let display = format_number(result);

        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "expression": expression,
                "result": result,
                "display": display,
            }),
            display: Some(display),
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

fn required_str<'a>(arguments: &'a serde_json::Value, name: &str) -> anyhow::Result<&'a str> {
    arguments
        .get(name)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("{name} is required"))
}

#[cfg(feature = "tools-http")]
async fn validate_fetch_url(url: &reqwest::Url) -> anyhow::Result<()> {
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            anyhow::bail!("unsupported url scheme '{scheme}'; only http and https are allowed")
        }
    }

    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("credentials in fetch URLs are not allowed");
    }

    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("url host is required"))?;
    let normalized_host = host.trim_end_matches('.').to_ascii_lowercase();
    if normalized_host == "localhost" || normalized_host.ends_with(".localhost") {
        anyhow::bail!("localhost URLs are not allowed");
    }
    if let Ok(ip) = normalized_host.parse::<IpAddr>() {
        ensure_public_ip(ip)?;
        return Ok(());
    }

    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("url port could not be inferred"))?;
    let addresses = lookup_host((host, port)).await?;
    let mut saw_address = false;
    for address in addresses {
        saw_address = true;
        ensure_public_ip(address.ip())?;
    }
    if !saw_address {
        anyhow::bail!("url host did not resolve");
    }

    Ok(())
}

#[cfg(feature = "tools-http")]
fn ensure_public_ip(ip: IpAddr) -> anyhow::Result<()> {
    let blocked = match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_unspecified()
                || ip.octets()[0] == 0
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_multicast()
                || ip.is_unspecified()
                || is_ipv6_unique_local(ip)
                || is_ipv6_unicast_link_local(ip)
        }
    };

    if blocked {
        anyhow::bail!("fetch blocked non-public address {ip}");
    }
    Ok(())
}

#[cfg(feature = "tools-http")]
fn is_ipv6_unique_local(ip: std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

#[cfg(feature = "tools-http")]
fn is_ipv6_unicast_link_local(ip: std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn eval_arithmetic(expression: &str) -> anyhow::Result<f64> {
    let mut parser = ArithmeticParser::new(expression);
    let value = parser.parse_expression()?;
    parser.skip_whitespace();
    if !parser.is_done() {
        anyhow::bail!("unexpected token '{}'", parser.peek().unwrap_or('\0'));
    }
    if !value.is_finite() {
        anyhow::bail!("calculation result is not finite");
    }
    Ok(value)
}

fn format_number(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        return format!("{value:.0}");
    }
    let formatted = format!("{value:.12}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

struct ArithmeticParser {
    chars: Vec<char>,
    pos: usize,
}

impl ArithmeticParser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn parse_expression(&mut self) -> anyhow::Result<f64> {
        let mut value = self.parse_term()?;
        loop {
            self.skip_whitespace();
            if self.consume('+') {
                value += self.parse_term()?;
            } else if self.consume('-') {
                value -= self.parse_term()?;
            } else {
                return Ok(value);
            }
        }
    }

    fn parse_term(&mut self) -> anyhow::Result<f64> {
        let mut value = self.parse_power()?;
        loop {
            self.skip_whitespace();
            if self.consume('*') {
                value *= self.parse_power()?;
            } else if self.consume('/') {
                let divisor = self.parse_power()?;
                if divisor == 0.0 {
                    anyhow::bail!("division by zero");
                }
                value /= divisor;
            } else {
                return Ok(value);
            }
        }
    }

    fn parse_power(&mut self) -> anyhow::Result<f64> {
        let base = self.parse_unary()?;
        self.skip_whitespace();
        if self.consume('^') {
            let exponent = self.parse_power()?;
            return Ok(base.powf(exponent));
        }
        Ok(base)
    }

    fn parse_unary(&mut self) -> anyhow::Result<f64> {
        self.skip_whitespace();
        if self.consume('+') {
            return self.parse_unary();
        }
        if self.consume('-') {
            return Ok(-self.parse_unary()?);
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> anyhow::Result<f64> {
        self.skip_whitespace();
        if self.consume('(') {
            let value = self.parse_expression()?;
            self.skip_whitespace();
            if !self.consume(')') {
                anyhow::bail!("missing closing parenthesis");
            }
            return Ok(value);
        }
        self.parse_number()
    }

    fn parse_number(&mut self) -> anyhow::Result<f64> {
        self.skip_whitespace();
        let start = self.pos;
        let mut has_digit = false;

        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                has_digit = true;
                self.pos += 1;
            } else if ch == '.' {
                self.pos += 1;
            } else {
                break;
            }
        }

        if matches!(self.peek(), Some('e' | 'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some('+' | '-')) {
                self.pos += 1;
            }
            let exponent_start = self.pos;
            while matches!(self.peek(), Some(ch) if ch.is_ascii_digit()) {
                self.pos += 1;
            }
            if self.pos == exponent_start {
                anyhow::bail!("invalid exponent");
            }
        }

        if !has_digit {
            anyhow::bail!("expected number");
        }

        self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .parse::<f64>()
            .map_err(|error| anyhow::anyhow!("invalid number: {error}"))
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(ch) if ch.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn consume(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn is_done(&self) -> bool {
        self.pos >= self.chars.len()
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
