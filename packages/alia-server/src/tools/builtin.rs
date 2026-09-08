//! BuiltinToolRegistry: Full tool registry with built-in tools and plugin support.
//!
//! ## Built-in Tools
//! - `get_current_time`: Get current date/time in specified timezone
//! - `fetch`: Fetch a public HTTP(S) URL and return bounded text content
//! - `calculate`: Evaluate a basic arithmetic expression
//! - `memory_search`: Search the user's Alia memory store
//! - `memory_store`: Store information to memory
//! - `schedule_task`: Create a scheduled task
//!
//! ## Plugin Tools
//! Plugins can register custom tools at runtime.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
#[cfg(feature = "tools-http")]
use std::{net::IpAddr, time::Duration};

use async_trait::async_trait;
use chrono::Utc;
use chrono_tz::Tz;
#[cfg(feature = "tools-http")]
use futures::StreamExt;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
    TransactionTrait,
};
#[cfg(feature = "tools-http")]
use tokio::net::lookup_host;
use uuid::Uuid;

use crate::entity::{registry_artifact, registry_dependency, skill_preferred_tool, tool};
use crate::event::{EventBus, ServerEvent};
use crate::memory_policy::{
    detect_sensitive_memory, memory_content_equivalent, memory_merge_key, merge_memory_patch,
    normalize_memory_layer,
};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    MemoryBackend, MemoryListOpts, MemoryNode, PreparedTool, RetrievalOpts, ScheduleConfig,
    ScheduledTask, SchedulerBackend, ToolCall, ToolContext, ToolDefinition, ToolExecutor,
    ToolRegistry, ToolResult, ToolRoutingEmbedding, ToolRoutingIndexEntry,
};
use crate::providers::sensor::SensorRegistry;
use crate::registry::{ArtifactRef, ArtifactRegistry, KIND_TOOL, is_plugin_source};
use crate::tools::sensor::{SensorListTool, SensorReadTool};

const FETCH_DEFAULT_MAX_BYTES: usize = 64 * 1024;
const FETCH_HARD_MAX_BYTES: usize = 1024 * 1024;
const FETCH_DEFAULT_TIMEOUT_SEC: u64 = 10;
const FETCH_HARD_TIMEOUT_SEC: u64 = 30;
const OBSOLETE_BUILTIN_TOOLS: &[&str] = &["device_control", "send_notification"];

struct ToolCatalogFields {
    routing_keywords: serde_json::Value,
    routing_examples: serde_json::Value,
    routing_text: String,
}

fn tool_catalog_fields(definition: &ToolDefinition) -> ToolCatalogFields {
    let schema_terms = schema_property_names(&definition.parameters).join(" ");
    let mut hints = tool_routing_hints(&definition.name)
        .iter()
        .map(|hint| (*hint).to_string())
        .collect::<Vec<_>>();
    hints.extend(definition.routing_hints.iter().cloned());
    let routing_text = format!(
        "{}\n{}\nsource: {}\nparameters: {}\nhints: {}",
        definition.name,
        definition.description,
        definition.source,
        schema_terms,
        hints.join(" ")
    );
    let mut keywords = tokenize_catalog_text(&routing_text);
    keywords.extend(hints);
    keywords.sort();
    keywords.dedup();

    ToolCatalogFields {
        routing_keywords: serde_json::json!(keywords),
        routing_examples: serde_json::json!([]),
        routing_text,
    }
}

fn set_tool_catalog(
    active: &mut tool::ActiveModel,
    definition: &ToolDefinition,
    preserve_existing_catalog: bool,
) {
    if preserve_existing_catalog {
        return;
    }

    let catalog = tool_catalog_fields(definition);
    active.routing_keywords = Set(catalog.routing_keywords);
    active.routing_examples = Set(catalog.routing_examples);
    active.routing_text = Set(catalog.routing_text);
    active.routing_embedding = Set(None);
}

fn preserve_existing_tool_catalog(row: &tool::Model, definition: &ToolDefinition) -> bool {
    if row.routing_text.trim().is_empty()
        || row.description != definition.description
        || row.parameters != definition.parameters
        || row.source != definition.source
    {
        return false;
    }

    if looks_like_generated_tool_catalog(&row.routing_text, definition) {
        let current = tool_catalog_fields(definition);
        return row.routing_text == current.routing_text;
    }

    true
}

fn looks_like_generated_tool_catalog(routing_text: &str, definition: &ToolDefinition) -> bool {
    routing_text.starts_with(&format!(
        "{}\n{}\nsource: {}\nparameters: ",
        definition.name, definition.description, definition.source
    )) && routing_text.contains("\nhints: ")
}

fn tokenize_catalog_text(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter_map(|term| {
            let term = term.trim();
            (term.len() >= 2).then(|| term.to_string())
        })
        .collect()
}

fn schema_property_names(parameters: &serde_json::Value) -> Vec<String> {
    parameters
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default()
}

fn routing_keywords_from_json(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn parse_tool_routing_embedding(value: Option<&serde_json::Value>) -> Option<ToolRoutingEmbedding> {
    let value = value?;
    let model = value
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    let vector_value = value.get("vector").unwrap_or(value);
    let vector = vector_value
        .as_array()?
        .iter()
        .map(|item| item.as_f64().map(|value| value as f32))
        .collect::<Option<Vec<_>>>()?;
    (!vector.is_empty()).then_some(ToolRoutingEmbedding { model, vector })
}

fn tool_routing_hints(tool_name: &str) -> &'static [&'static str] {
    match tool_name {
        "get_current_time" => &[
            "time", "date", "today", "now", "时间", "几点", "今天", "日期",
        ],
        "fetch" => &[
            "fetch", "url", "http", "https", "web", "search", "lookup", "latest", "current",
            "网页", "链接", "搜索", "查询", "最新", "当前",
        ],
        "calculate" => &["calculate", "compute", "math", "算", "计算"],
        "memory_search" => &["remember", "memory", "recall", "记得", "记忆", "之前"],
        "memory_store" => &[
            "remember this",
            "preference",
            "save",
            "store",
            "记住",
            "保存",
            "记录",
        ],
        "memory_forget" => &[
            "forget",
            "delete memory",
            "remove memory",
            "忘记",
            "删除记忆",
            "移除记忆",
        ],
        _ => &[],
    }
}

/// BuiltinToolRegistry: Full implementation of ToolRegistry
#[derive(Default)]
struct ExecutorCatalog {
    entries: HashMap<String, Arc<dyn ToolExecutor>>,
    revisions: HashMap<String, String>,
}

impl std::ops::Deref for ExecutorCatalog {
    type Target = HashMap<String, Arc<dyn ToolExecutor>>;
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl ExecutorCatalog {
    fn insert(
        &mut self,
        name: String,
        executor: Arc<dyn ToolExecutor>,
    ) -> Option<Arc<dyn ToolExecutor>> {
        let revision = if self.revisions.contains_key(&name) {
            Uuid::new_v4().to_string()
        } else {
            "initial".into()
        };
        self.revisions.insert(name.clone(), revision);
        self.entries.insert(name, executor)
    }

    fn remove(&mut self, name: &str) -> Option<Arc<dyn ToolExecutor>> {
        // Retain the revision tombstone so remove + re-register cannot restore
        // the identity of an earlier approval in this process.
        self.entries.remove(name)
    }
}

pub struct BuiltinToolRegistry {
    /// Code-backed tools. Built-ins, Rust components, and in-process Rust
    /// plugins all live here; DB rows are only the discoverable catalog.
    executors: Arc<RwLock<ExecutorCatalog>>,
    /// Event bus for publishing tool events
    event_bus: tokio::sync::Mutex<Option<EventBus>>,
    /// DB connection for persisting plugin tools
    db: Option<DatabaseConnection>,
    /// Unified artifact/dependency graph manager.
    artifacts: Option<Arc<ArtifactRegistry>>,
    /// Memory backend used by memory_search and memory_store built-in tools.
    memory: Option<Arc<dyn MemoryBackend>>,
    /// Scheduler backend used by schedule_task.
    scheduler: Option<Arc<dyn SchedulerBackend>>,
}

impl BuiltinToolRegistry {
    pub fn new() -> Self {
        Self {
            executors: Arc::default(),
            event_bus: tokio::sync::Mutex::new(None),
            db: None,
            artifacts: None,
            memory: None,
            scheduler: None,
        }
    }

    /// Create with sensor registry integration (sensor_read + sensor_list tools).
    pub fn with_sensors(sensors: Arc<SensorRegistry>) -> Self {
        let this = Self::new();
        this.register_executor(Arc::new(SensorReadTool::new(sensors.clone())));
        this.register_executor(Arc::new(SensorListTool::new(sensors)));
        this
    }

    fn register_builtins(&self) {
        // get_current_time
        self.executors
            .write()
            .unwrap()
            .insert("get_current_time".to_string(), Arc::new(GetCurrentTime));

        // fetch
        #[cfg(feature = "tools-http")]
        self.executors
            .write()
            .unwrap()
            .insert("fetch".to_string(), Arc::new(FetchUrl));

        // calculate
        self.executors
            .write()
            .unwrap()
            .insert("calculate".to_string(), Arc::new(Calculate));

        if let Some(memory) = &self.memory {
            // memory_search
            self.executors.write().unwrap().insert(
                "memory_search".to_string(),
                Arc::new(MemorySearch::new(Some(memory.clone()))),
            );

            // memory_store
            self.executors.write().unwrap().insert(
                "memory_store".to_string(),
                Arc::new(MemoryStore::new(Some(memory.clone()))),
            );

            self.executors.write().unwrap().insert(
                "memory_forget".to_string(),
                Arc::new(MemoryForget::new(Some(memory.clone()))),
            );
        } else {
            tracing::info!("memory system unavailable; memory tools are not registered");
        }

        // schedule_task
        if let Some(scheduler) = &self.scheduler {
            self.executors.write().unwrap().insert(
                "schedule_task".to_string(),
                Arc::new(ScheduleTask::new(scheduler.clone())),
            );
        } else {
            tracing::info!("scheduler unavailable; schedule_task tool is not registered");
        }
    }

    /// Register an additional tool executor at runtime (for tools that need external state).
    pub fn register_executor(&self, executor: Arc<dyn ToolExecutor>) {
        let name = executor.definition().name;
        self.executors.write().unwrap().insert(name, executor);
    }

    fn artifacts(&self) -> anyhow::Result<&ArtifactRegistry> {
        self.artifacts
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("BuiltinToolRegistry not initialized"))
    }

    /// Persist built-in tool definitions so skills can prefer them through
    /// `skill_preferred_tools` with real foreign keys. The in-memory executor
    /// remains authoritative for execution.
    async fn upsert_executor_tools_to_db(&self) -> anyhow::Result<()> {
        let Some(db) = &self.db else { return Ok(()) };
        let now = Utc::now();

        let definitions = self
            .executors
            .read()
            .unwrap()
            .values()
            .map(|executor| executor.definition())
            .collect::<Vec<_>>();

        for definition in definitions {
            let artifact = ArtifactRef::tool(definition.name.clone());
            let existing = tool::Entity::find()
                .filter(tool::Column::Name.eq(&definition.name))
                .one(db)
                .await?;

            match existing {
                Some(row) => {
                    let preserve_catalog = preserve_existing_tool_catalog(&row, &definition);
                    let mut active: tool::ActiveModel = row.into();
                    active.description = Set(definition.description.clone());
                    active.parameters = Set(definition.parameters.clone());
                    active.source = Set(definition.source.clone());
                    active.require_confirmation = Set(definition.require_confirmation);
                    active.required_role = Set(definition.required_role.clone());
                    set_tool_catalog(&mut active, &definition, preserve_catalog);
                    active.enabled = Set(true);
                    active.updated_at = Set(now);
                    active.update(db).await?;
                }
                None => {
                    let catalog = tool_catalog_fields(&definition);
                    tool::ActiveModel {
                        id: Set(Uuid::new_v4()),
                        name: Set(definition.name),
                        description: Set(definition.description),
                        parameters: Set(definition.parameters),
                        source: Set(definition.source.clone()),
                        require_confirmation: Set(definition.require_confirmation),
                        required_role: Set(definition.required_role),
                        routing_keywords: Set(catalog.routing_keywords),
                        routing_examples: Set(catalog.routing_examples),
                        routing_text: Set(catalog.routing_text),
                        routing_embedding: Set(None),
                        enabled: Set(true),
                        created_at: Set(now),
                        updated_at: Set(now),
                    }
                    .insert(db)
                    .await?;
                }
            }
            self.artifacts()?
                .upsert_artifact_in(db, &artifact, &definition.source)
                .await?;
        }

        self.artifacts()?.sync_existing_tool_artifacts().await?;
        Ok(())
    }

    async fn remove_obsolete_builtin_tools(&self) -> anyhow::Result<()> {
        let Some(db) = &self.db else { return Ok(()) };
        let tx = db.begin().await?;
        let tool_names = OBSOLETE_BUILTIN_TOOLS.to_vec();
        let obsolete_tool_ids = tool::Entity::find()
            .filter(tool::Column::Source.eq("builtin"))
            .filter(tool::Column::Name.is_in(tool_names.clone()))
            .all(&tx)
            .await?
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>();

        if !obsolete_tool_ids.is_empty() {
            skill_preferred_tool::Entity::delete_many()
                .filter(skill_preferred_tool::Column::ToolId.is_in(obsolete_tool_ids))
                .exec(&tx)
                .await?;
        }
        registry_dependency::Entity::delete_many()
            .filter(registry_dependency::Column::DependentKind.eq(KIND_TOOL))
            .filter(registry_dependency::Column::DependentName.is_in(tool_names.clone()))
            .exec(&tx)
            .await?;
        registry_dependency::Entity::delete_many()
            .filter(registry_dependency::Column::DependencyKind.eq(KIND_TOOL))
            .filter(registry_dependency::Column::DependencyName.is_in(tool_names.clone()))
            .exec(&tx)
            .await?;

        let tool_delete = tool::Entity::delete_many()
            .filter(tool::Column::Source.eq("builtin"))
            .filter(tool::Column::Name.is_in(tool_names.clone()))
            .exec(&tx)
            .await?;
        let artifact_delete = registry_artifact::Entity::delete_many()
            .filter(registry_artifact::Column::ArtifactKind.eq(KIND_TOOL))
            .filter(registry_artifact::Column::ArtifactName.is_in(tool_names))
            .filter(registry_artifact::Column::OwnerKind.eq("builtin"))
            .exec(&tx)
            .await?;
        tx.commit().await?;

        if tool_delete.rows_affected > 0 || artifact_delete.rows_affected > 0 {
            tracing::info!(
                tools = ?OBSOLETE_BUILTIN_TOOLS,
                rows = tool_delete.rows_affected,
                artifacts = artifact_delete.rows_affected,
                "removed obsolete built-in noop tools"
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
        self.artifacts = Some(ctx.artifacts.clone());
        self.memory = ctx.memory.clone();
        self.scheduler = ctx.scheduler.clone();
        if let Err(e) = self.remove_obsolete_builtin_tools().await {
            tracing::warn!(error = %e, "failed to remove obsolete built-in noop tools");
        }
        self.register_builtins();

        if let Err(e) = self.upsert_executor_tools_to_db().await {
            tracing::warn!(error = %e, "failed to persist executor tool definitions");
        }

        tracing::info!(
            executor_count = self.executors.read().unwrap().len(),
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
    async fn resolve_executor(&self, name: &str) -> anyhow::Result<Option<PreparedTool>> {
        let catalog = self.executors.read().unwrap();
        Ok(catalog.entries.get(name).map(|executor| PreparedTool {
            executor: executor.clone(),
            registration_revision: catalog.revisions[name].clone(),
        }))
    }
    async fn list_tools(&self) -> anyhow::Result<Vec<ToolDefinition>> {
        let tools: Vec<ToolDefinition> = self
            .executors
            .read()
            .unwrap()
            .values()
            .map(|executor| executor.definition())
            .collect();

        Ok(tools)
    }

    async fn get_tool(&self, name: &str) -> anyhow::Result<Option<ToolDefinition>> {
        // Check code-backed executors first
        if let Some(executor) = self.executors.read().unwrap().get(name) {
            return Ok(Some(executor.definition()));
        }

        Ok(None)
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
        let executor = self.executors.read().unwrap().get(&call.tool_name).cloned();
        let result = if let Some(executor) = executor {
            executor.execute(call.arguments, ctx).await
        } else {
            Ok(ToolResult {
                call_id: call_id.clone(),
                success: false,
                output: serde_json::json!({"error": format!("unknown tool: {}", call.tool_name)}),
                display: Some(format!("Unknown tool: {}", call.tool_name)),
            })
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
        anyhow::bail!(
            "metadata-only plugin tool '{}' is not executable; register a code-backed ToolExecutor with register_tool_executor",
            tool.name
        )
    }

    async fn register_tool_executor(
        &self,
        owner_source: &str,
        executor: Arc<dyn ToolExecutor>,
    ) -> anyhow::Result<()> {
        let definition = executor.definition();
        if definition.source != owner_source {
            anyhow::bail!(
                "tool executor '{}' source mismatch: definition has '{}', owner source is '{}'",
                definition.name,
                definition.source,
                owner_source
            );
        }

        if let Some(db) = &self.db {
            let now = Utc::now();
            let artifact = ArtifactRef::tool(definition.name.clone());
            let existing = tool::Entity::find()
                .filter(tool::Column::Name.eq(&definition.name))
                .one(db)
                .await?;

            match existing {
                Some(row) => {
                    let preserve_catalog = preserve_existing_tool_catalog(&row, &definition);
                    let mut active: tool::ActiveModel = row.into();
                    active.description = Set(definition.description.clone());
                    active.parameters = Set(definition.parameters.clone());
                    active.source = Set(definition.source.clone());
                    active.require_confirmation = Set(definition.require_confirmation);
                    active.required_role = Set(definition.required_role.clone());
                    set_tool_catalog(&mut active, &definition, preserve_catalog);
                    active.enabled = Set(true);
                    active.updated_at = Set(now);
                    active.update(db).await?;
                }
                None => {
                    let catalog = tool_catalog_fields(&definition);
                    tool::ActiveModel {
                        id: Set(Uuid::new_v4()),
                        name: Set(definition.name.clone()),
                        description: Set(definition.description.clone()),
                        parameters: Set(definition.parameters.clone()),
                        source: Set(definition.source.clone()),
                        require_confirmation: Set(definition.require_confirmation),
                        required_role: Set(definition.required_role.clone()),
                        routing_keywords: Set(catalog.routing_keywords),
                        routing_examples: Set(catalog.routing_examples),
                        routing_text: Set(catalog.routing_text),
                        routing_embedding: Set(None),
                        enabled: Set(true),
                        created_at: Set(now),
                        updated_at: Set(now),
                    }
                    .insert(db)
                    .await?;
                }
            }
            self.artifacts()?
                .upsert_artifact_in(db, &artifact, &definition.source)
                .await?;
        }

        self.executors
            .write()
            .unwrap()
            .insert(definition.name.clone(), executor);
        tracing::info!(
            tool = %definition.name,
            source = %definition.source,
            "registered code-backed tool executor"
        );
        Ok(())
    }

    async fn unregister_plugin_tool(&self, tool_name: &str) -> anyhow::Result<()> {
        let executor_source = self
            .executors
            .read()
            .unwrap()
            .get(tool_name)
            .map(|executor| executor.definition().source);
        if let Some(source) = &executor_source
            && !is_plugin_source(source)
        {
            anyhow::bail!("cannot unregister non-plugin executor-backed tool: {tool_name}");
        }

        if let Some(db) = &self.db {
            let tx = db.begin().await?;
            let artifact = ArtifactRef::tool(tool_name.to_string());
            self.artifacts()?
                .ensure_no_external_dependents_in(&tx, std::slice::from_ref(&artifact))
                .await?;
            tool::Entity::delete_many()
                .filter(tool::Column::Name.eq(tool_name))
                .exec(&tx)
                .await?;
            self.artifacts()?.delete_artifact_in(&tx, &artifact).await?;
            tx.commit().await?;
        }

        let executor_removed = self.executors.write().unwrap().remove(tool_name).is_some();
        if !executor_removed {
            anyhow::bail!("plugin tool not found: {tool_name}");
        }
        tracing::info!(tool = %tool_name, "unregistered plugin tool");
        Ok(())
    }

    async fn unregister_plugin_tools(&self, plugin_name: &str) -> anyhow::Result<u64> {
        let Some(db) = &self.db else { return Ok(0) };
        let tx = db.begin().await?;
        let artifacts = self
            .artifacts()?
            .owned_artifacts_in(&tx, "plugin", plugin_name, Some(KIND_TOOL))
            .await?;

        self.artifacts()?
            .ensure_no_external_dependents_in(&tx, &artifacts)
            .await?;

        let mut removed = 0;
        for artifact in &artifacts {
            let result = tool::Entity::delete_many()
                .filter(tool::Column::Name.eq(&artifact.name))
                .exec(&tx)
                .await?;
            removed += result.rows_affected;
            self.artifacts()?.delete_artifact_in(&tx, artifact).await?;
        }

        tx.commit().await?;

        for artifact in &artifacts {
            self.executors.write().unwrap().remove(&artifact.name);
        }

        tracing::info!(plugin = %plugin_name, removed, "unregistered plugin tools");
        Ok(removed)
    }

    async fn routing_index(
        &self,
        tool_names: &[String],
    ) -> anyhow::Result<Vec<ToolRoutingIndexEntry>> {
        let Some(db) = &self.db else {
            return Ok(Vec::new());
        };
        if tool_names.is_empty() {
            return Ok(Vec::new());
        }

        let rows = tool::Entity::find()
            .filter(tool::Column::Enabled.eq(true))
            .filter(tool::Column::Name.is_in(tool_names.iter().cloned()))
            .all(db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| ToolRoutingIndexEntry {
                name: row.name,
                routing_text: row.routing_text,
                routing_keywords: routing_keywords_from_json(&row.routing_keywords),
                routing_embedding: parse_tool_routing_embedding(row.routing_embedding.as_ref()),
            })
            .collect())
    }

    async fn update_routing_embedding(
        &self,
        tool_name: &str,
        embedding_model: &str,
        embedding: &[f32],
    ) -> anyhow::Result<()> {
        let Some(db) = &self.db else {
            return Ok(());
        };
        if tool_name.trim().is_empty() || embedding.is_empty() {
            return Ok(());
        }

        let Some(row) = tool::Entity::find()
            .filter(tool::Column::Name.eq(tool_name))
            .one(db)
            .await?
        else {
            return Ok(());
        };

        let mut active: tool::ActiveModel = row.into();
        active.routing_embedding = Set(Some(serde_json::json!({
            "schema": "alia.tool_routing_embedding.v1",
            "model": embedding_model,
            "dimension": embedding.len(),
            "vector": embedding,
            "updated_at": Utc::now().to_rfc3339(),
        })));
        active.updated_at = Set(Utc::now());
        active.update(db).await?;
        Ok(())
    }
}

// ============ Built-in Tool Implementations ============

/// Get current date/time
pub struct GetCurrentTime;

#[async_trait]
impl ToolExecutor for GetCurrentTime {
    fn implementation_revision(&self) -> Option<String> {
        Some("alia.builtin.GetCurrentTime".into())
    }
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
            routing_hints: Vec::new(),
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
        let timezone = parse_time_tool_timezone(arguments.get("timezone"))?;

        let now = Utc::now();
        let local_now = now.with_timezone(&timezone);

        let time_str = match format {
            "iso8601" => local_now.to_rfc3339(),
            "unix" => now.timestamp().to_string(),
            _ => local_now.format("%Y-%m-%d %H:%M:%S %Z (%:z)").to_string(),
        };

        Ok(ToolResult {
            call_id: String::new(), // Will be filled by caller
            success: true,
            output: serde_json::json!({
                "datetime": time_str,
                "timezone": timezone.name(),
                "utc_datetime": now.to_rfc3339(),
                "utc_offset": local_now.format("%:z").to_string(),
                "timezone_abbreviation": local_now.format("%Z").to_string(),
                "timestamp": now.timestamp()
            }),
            display: Some(time_str),
        })
    }
}

fn parse_time_tool_timezone(value: Option<&serde_json::Value>) -> anyhow::Result<Tz> {
    let Some(raw) = value else {
        return Ok(chrono_tz::UTC);
    };
    let Some(timezone) = raw
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        anyhow::bail!("timezone must be an IANA timezone string, for example 'Asia/Shanghai'");
    };

    match timezone.to_ascii_uppercase().as_str() {
        "UTC" | "GMT" | "Z" => return Ok(chrono_tz::UTC),
        _ => {}
    }

    timezone.parse::<Tz>().map_err(|_| {
        anyhow::anyhow!(
            "invalid timezone '{timezone}'; expected an IANA timezone name such as 'Asia/Shanghai', 'America/Los_Angeles', or 'Europe/London'"
        )
    })
}

/// Fetch a public HTTP(S) URL.
#[cfg(feature = "tools-http")]
pub struct FetchUrl;

#[cfg(feature = "tools-http")]
#[async_trait]
impl ToolExecutor for FetchUrl {
    fn implementation_revision(&self) -> Option<String> {
        Some("alia.builtin.FetchUrl".into())
    }
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
            routing_hints: Vec::new(),
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
    fn implementation_revision(&self) -> Option<String> {
        Some("alia.builtin.Calculate".into())
    }
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
            routing_hints: Vec::new(),
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
pub struct MemorySearch {
    memory: Option<Arc<dyn MemoryBackend>>,
}

impl MemorySearch {
    pub fn new(memory: Option<Arc<dyn MemoryBackend>>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl ToolExecutor for MemorySearch {
    fn implementation_revision(&self) -> Option<String> {
        Some("alia.builtin.MemorySearch".into())
    }
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
                        "description": "Memory layers to search: 'core', 'episodic', 'procedural'. Defaults to all."
                    }
                },
                "required": ["query"]
            }),
            routing_hints: Vec::new(),
            source: "builtin".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let query = required_str(&arguments, "query")?;
        let Some(memory) = &self.memory else {
            return Ok(ToolResult {
                call_id: String::new(),
                success: false,
                output: serde_json::json!({"error": "memory_backend_unavailable"}),
                display: Some("Memory backend is not available".to_string()),
            });
        };

        let max_results = arguments
            .get("max_results")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(5)
            .clamp(1, 20) as u32;
        let layers = parse_memory_layers(arguments.get("layers"))?;
        let opts = RetrievalOpts {
            max_results: Some(max_results),
            min_similarity: None,
            layers,
        };
        let mut results = memory.retrieve(ctx.user_id, query, opts.clone()).await?;
        if results.is_empty() {
            results = retrieve_memory_by_terms(memory.as_ref(), ctx.user_id, query, &opts).await?;
        }
        let result_count = results.len();
        let output_results = results
            .iter()
            .map(|node| {
                serde_json::json!({
                    "id": node.id,
                    "layer": node.memory_layer,
                    "type": node.node_type,
                    "content": node.content,
                    "importance": node.importance,
                    "metadata": node.metadata,
                })
            })
            .collect::<Vec<_>>();
        let display = if result_count == 0 {
            "No matching memories found".to_string()
        } else {
            format!(
                "Found {result_count} matching memor{}",
                if result_count == 1 { "y" } else { "ies" }
            )
        };

        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "query": query,
                "results": output_results,
                "count": result_count,
            }),
            display: Some(display),
        })
    }
}

/// Store information to memory
pub struct MemoryStore {
    memory: Option<Arc<dyn MemoryBackend>>,
}

impl MemoryStore {
    pub fn new(memory: Option<Arc<dyn MemoryBackend>>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl ToolExecutor for MemoryStore {
    fn implementation_revision(&self) -> Option<String> {
        Some("alia.builtin.MemoryStore".into())
    }
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "memory_store".to_string(),
            description: "Store information only when the user explicitly asks the assistant to remember it. Do not store secrets, identifiers, health details, religious views, or political views from chat; sensitive memories must be added by the user in the memory UI/API.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "The information to remember"
                    },
                    "layer": {
                        "type": "string",
                        "enum": ["core", "preference", "relationship", "episodic", "procedural"],
                        "description": "Memory layer to store in. Defaults to 'episodic'."
                    },
                    "importance": {
                        "type": "number",
                        "description": "Importance score from 0.0 to 1.0. Defaults to 0.5."
                    },
                    "node_type": {
                        "type": "string",
                        "description": "Memory node type, for example 'fact', 'preference', or 'event'. Defaults to 'fact'."
                    }
                },
                "required": ["content"]
            }),
            routing_hints: Vec::new(),
            source: "builtin".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let content = required_str(&arguments, "content")?.trim().to_string();
        if let Some(sensitive) = detect_sensitive_memory(&content) {
            return Ok(ToolResult {
                call_id: String::new(),
                success: false,
                output: serde_json::json!({
                    "error": "sensitive_memory_blocked",
                    "category": sensitive.category,
                    "reason": sensitive.reason,
                    "message": "Sensitive memories must be added directly by the user in the memory UI/API."
                }),
                display: Some("I cannot store sensitive memory from chat. Add it directly in memory settings if you want it saved.".to_string()),
            });
        }
        let Some(memory) = &self.memory else {
            return Ok(ToolResult {
                call_id: String::new(),
                success: false,
                output: serde_json::json!({"error": "memory_backend_unavailable"}),
                display: Some("Memory backend is not available".to_string()),
            });
        };

        let layer = arguments
            .get("layer")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("episodic")
            .trim()
            .to_lowercase();
        let layer = normalize_memory_layer(&layer);
        let importance = arguments
            .get("importance")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.5)
            .clamp(0.0, 1.0) as f32;
        let node_type = arguments
            .get("node_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("fact")
            .trim()
            .to_string();
        let mut incoming = MemoryNode::new(
            layer.clone(),
            node_type.clone(),
            content.clone(),
            importance,
        )
        .with_metadata(serde_json::json!({
            "source": ctx.source,
            "channel_id": ctx.channel_id,
            "conversation_id": ctx.conversation_id,
            "stored_by": "memory_store_tool",
        }))
        .with_runtime_fields("semantic", "user", 1.0, importance, "user_visible");
        incoming.evidence = serde_json::json!({
            "source": "direct_memory_tool",
            "summary": "The user explicitly asked the assistant to remember this.",
        });
        let action = upsert_tool_memory(memory.as_ref(), ctx, &incoming).await?;

        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "stored": action != "duplicate",
                "action": action,
                "layer": layer,
                "type": node_type,
                "content": content,
                "importance": importance,
            }),
            display: Some(match action {
                "updated" => "Memory updated".to_string(),
                "duplicate" => "That memory is already saved".to_string(),
                _ => "Memory stored".to_string(),
            }),
        })
    }
}

/// Delete information from memory when the user explicitly asks to forget it.
pub struct MemoryForget {
    memory: Option<Arc<dyn MemoryBackend>>,
}

impl MemoryForget {
    pub fn new(memory: Option<Arc<dyn MemoryBackend>>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl ToolExecutor for MemoryForget {
    fn implementation_revision(&self) -> Option<String> {
        Some("alia.builtin.MemoryForget".into())
    }
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "memory_forget".to_string(),
            description: "Forget/delete saved user memories only when the user explicitly asks to forget something. Provide either an exact memory id, or a text query describing the memory to forget.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Exact memory id to delete when known."
                    },
                    "query": {
                        "type": "string",
                        "description": "Text query describing the memory to forget."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum matching memories to delete. Defaults to 1, maximum 10."
                    }
                }
            }),
            routing_hints: Vec::new(),
            source: "builtin".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let Some(memory) = &self.memory else {
            return Ok(ToolResult {
                call_id: String::new(),
                success: false,
                output: serde_json::json!({"error": "memory_backend_unavailable"}),
                display: Some("Memory backend is not available".to_string()),
            });
        };

        if let Some(id) = arguments
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            if memory.get(ctx.user_id, id).await?.is_none() {
                return Ok(ToolResult {
                    call_id: String::new(),
                    success: false,
                    output: serde_json::json!({"error": "memory_not_found", "id": id}),
                    display: Some("Memory not found".to_string()),
                });
            }
            memory.delete(ctx.user_id, id).await?;
            return Ok(ToolResult {
                call_id: String::new(),
                success: true,
                output: serde_json::json!({"deleted": 1, "ids": [id]}),
                display: Some("Memory forgotten".to_string()),
            });
        }

        let query = required_str(&arguments, "query")?;
        let max_results = arguments
            .get("max_results")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1)
            .clamp(1, 10) as u32;
        let opts = RetrievalOpts {
            max_results: Some(max_results),
            min_similarity: None,
            layers: None,
        };
        let mut matches = memory.retrieve(ctx.user_id, query, opts.clone()).await?;
        if matches.is_empty() {
            matches = retrieve_memory_by_terms(memory.as_ref(), ctx.user_id, query, &opts).await?;
        }
        let mut deleted = Vec::new();
        for node in matches.into_iter().take(max_results as usize) {
            if let Some(id) = node.id {
                memory.delete(ctx.user_id, &id).await?;
                deleted.push(id);
            }
        }
        let deleted_count = deleted.len();

        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "deleted": deleted_count,
                "ids": deleted,
                "query": query,
            }),
            display: Some(if deleted_count == 0 {
                "No matching memories found".to_string()
            } else {
                "Memory forgotten".to_string()
            }),
        })
    }
}

async fn upsert_tool_memory(
    memory: &dyn MemoryBackend,
    ctx: &ToolContext,
    incoming: &MemoryNode,
) -> anyhow::Result<&'static str> {
    anyhow::ensure!(
        ctx.source != "ai" || ctx.source_event_id.is_some(),
        "AI memory writes require a persisted source user message"
    );
    let mut context = memory.write_snapshot(ctx.user_id).await?;
    context.source_received_at = ctx.source_received_at;
    context.source_event_ids = ctx.source_event_id.into_iter().collect();
    anyhow::ensure!(
        context.source_received_at > context.source_cutoff_at,
        "memory source predates the user's deletion cutoff"
    );
    let merge_key = memory_merge_key(
        &incoming.memory_layer,
        &incoming.node_type,
        &incoming.content,
    );
    let existing = memory
        .list(
            ctx.user_id,
            MemoryListOpts {
                max_results: Some(200),
                layers: Some(vec![incoming.memory_layer.clone()]),
            },
        )
        .await?;
    for node in existing {
        if memory_content_equivalent(&node.content, &incoming.content) {
            return Ok("duplicate");
        }
        let Some(existing_id) = node.id.as_deref() else {
            continue;
        };
        if merge_key.is_some()
            && merge_key == memory_merge_key(&node.memory_layer, &node.node_type, &node.content)
        {
            memory
                .update_guarded(&context, existing_id, merge_memory_patch(&node, incoming))
                .await?;
            return Ok("updated");
        }
    }
    memory.store_guarded(&context, incoming.clone()).await?;
    Ok("stored")
}

/// Schedule a task
struct ScheduleTask {
    scheduler: Arc<dyn SchedulerBackend>,
}

impl ScheduleTask {
    fn new(scheduler: Arc<dyn SchedulerBackend>) -> Self {
        Self { scheduler }
    }
}

#[async_trait]
impl ToolExecutor for ScheduleTask {
    fn implementation_revision(&self) -> Option<String> {
        Some("alia.builtin.ScheduleTask".into())
    }
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
                        "enum": ["cron", "once", "interval", "condition"],
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
                    "poll_interval_sec": {
                        "type": "integer",
                        "description": "Polling interval in seconds for condition schedules"
                    },
                    "conditions": {
                        "type": ["array", "object"],
                        "description": "Optional condition expression. Arrays are AND; objects can use all/any/not, sensor, time_range, event_field, or plugin-defined types."
                    },
                    "action": {
                        "type": "object",
                        "description": "Action to execute when triggered"
                    }
                },
                "required": ["name", "schedule_type", "action"]
            }),
            routing_hints: Vec::new(),
            source: "builtin".to_string(),
            require_confirmation: true,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("name is required"))?;
        let schedule_type = arguments
            .get("schedule_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("schedule_type is required"))?;
        let action = arguments
            .get("action")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("action is required"))?;

        let task = ScheduledTask {
            id: Uuid::new_v4(),
            user_id: ctx.user_id,
            name: name.to_string(),
            enabled: true,
            schedule: ScheduleConfig {
                schedule_type: schedule_type.to_string(),
                cron_expression: arguments
                    .get("cron_expression")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
                run_at: arguments
                    .get("run_at")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
                every_sec: arguments
                    .get("every_sec")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32),
                poll_interval_sec: arguments
                    .get("poll_interval_sec")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32),
            },
            conditions: arguments
                .get("conditions")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
            action,
            cooldown_sec: arguments
                .get("cooldown_sec")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            max_retries: arguments
                .get("max_retries")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            created_by: ctx.source.clone(),
        };
        let task_id = self.scheduler.create_task(task.clone()).await?;

        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "scheduled": true,
                "task_id": task_id,
                "name": task.name,
                "schedule": task.schedule,
            }),
            display: Some(format!("Scheduled task '{name}'")),
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

fn parse_memory_layers(value: Option<&serde_json::Value>) -> anyhow::Result<Option<Vec<String>>> {
    let Some(value) = value else {
        return Ok(None);
    };

    let layers = match value {
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(|layer| normalize_memory_layer(layer.trim()))
            .collect::<Vec<_>>(),
        serde_json::Value::String(layer) => vec![normalize_memory_layer(layer.trim())],
        _ => anyhow::bail!("layers must be a string or an array of strings"),
    };

    let layers = layers
        .into_iter()
        .filter(|layer| !layer.is_empty())
        .collect::<Vec<_>>();
    if layers.is_empty() {
        Ok(None)
    } else {
        Ok(Some(layers))
    }
}

async fn retrieve_memory_by_terms(
    memory: &dyn MemoryBackend,
    user_id: Uuid,
    query: &str,
    opts: &RetrievalOpts,
) -> anyhow::Result<Vec<MemoryNode>> {
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for term in memory_query_terms(query) {
        for node in memory.retrieve(user_id, &term, opts.clone()).await? {
            let key = node
                .id
                .as_deref()
                .map(|id| id.to_string())
                .unwrap_or_else(|| node.content.clone());
            if seen.insert(key) {
                results.push(node);
            }
            if let Some(limit) = opts.max_results
                && results.len() >= limit as usize
            {
                return Ok(results);
            }
        }
    }

    Ok(results)
}

fn memory_query_terms(query: &str) -> Vec<String> {
    query
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | '，' | '?' | '？'))
        .map(str::trim)
        .filter(|term| term.chars().count() >= 2)
        .map(ToOwned::to_owned)
        .collect()
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

#[cfg(test)]
mod catalog_tests {
    use super::*;

    fn definition() -> ToolDefinition {
        ToolDefinition {
            name: "memory_store".to_string(),
            description: "Store information to memory".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "content": {"type": "string"}
                }
            }),
            routing_hints: vec!["favorite".to_string()],
            source: "builtin".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    fn existing_tool(definition: &ToolDefinition) -> tool::Model {
        let now = Utc::now();
        tool::Model {
            id: Uuid::new_v4(),
            name: definition.name.clone(),
            description: definition.description.clone(),
            parameters: definition.parameters.clone(),
            source: definition.source.clone(),
            require_confirmation: definition.require_confirmation,
            required_role: definition.required_role.clone(),
            routing_keywords: serde_json::json!(["custom"]),
            routing_examples: serde_json::json!(["custom example"]),
            routing_text: "custom operator maintained routing text".to_string(),
            routing_embedding: Some(serde_json::json!({
                "model": "test",
                "vector": [1.0, 0.0],
            })),
            enabled: true,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn catalog_generation_includes_default_hints_and_schema_terms() {
        let catalog = tool_catalog_fields(&definition());

        assert!(catalog.routing_text.contains("memory_store"));
        assert!(catalog.routing_text.contains("content"));
        assert!(catalog.routing_text.contains("favorite"));
        assert!(
            catalog
                .routing_keywords
                .as_array()
                .is_some_and(|keywords| keywords.iter().any(|value| value == "favorite"))
        );
    }

    #[test]
    fn unchanged_tool_preserves_operator_catalog_metadata() {
        let definition = definition();
        let existing = existing_tool(&definition);

        assert!(preserve_existing_tool_catalog(&existing, &definition));
    }

    #[test]
    fn changed_tool_definition_rebuilds_catalog_and_clears_embedding() {
        let mut definition = definition();
        let existing = existing_tool(&definition);
        definition.description = "Store explicit memories".to_string();
        let mut active: tool::ActiveModel = existing.into();

        assert!(!preserve_existing_tool_catalog(
            &tool::Model {
                id: Uuid::new_v4(),
                name: definition.name.clone(),
                description: "Store information to memory".to_string(),
                parameters: definition.parameters.clone(),
                source: definition.source.clone(),
                require_confirmation: false,
                required_role: None,
                routing_keywords: serde_json::json!(["custom"]),
                routing_examples: serde_json::json!(["custom example"]),
                routing_text: "custom operator maintained routing text".to_string(),
                routing_embedding: Some(serde_json::json!({"model": "test", "vector": [1.0]})),
                enabled: true,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            &definition
        ));
        set_tool_catalog(&mut active, &definition, false);

        assert_eq!(active.routing_embedding, Set(None));
        assert_ne!(
            active.routing_text,
            Set("custom operator maintained routing text".to_string())
        );
    }

    #[test]
    fn generated_catalog_rebuilds_when_routing_hints_change() {
        let mut old_definition = definition();
        old_definition.routing_hints = vec!["favorite".to_string()];
        let mut existing = existing_tool(&old_definition);
        existing.routing_text = tool_catalog_fields(&old_definition).routing_text;
        existing.routing_keywords = tool_catalog_fields(&old_definition).routing_keywords;
        existing.routing_embedding = Some(serde_json::json!({"model": "test", "vector": [1.0]}));

        let mut new_definition = old_definition.clone();
        new_definition.routing_hints.push("preference".to_string());
        let mut active: tool::ActiveModel = existing.clone().into();

        assert!(!preserve_existing_tool_catalog(&existing, &new_definition));
        set_tool_catalog(&mut active, &new_definition, false);

        assert_eq!(active.routing_embedding, Set(None));
        assert_ne!(active.routing_text, Set(existing.routing_text));
    }
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
