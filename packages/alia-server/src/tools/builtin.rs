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

use crate::modules::memory::MemoryService;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use anyhow::Context;
use async_trait::async_trait;
use chrono::Utc;
use chrono_tz::Tz;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
    TransactionTrait,
};
use uuid::Uuid;

use crate::entity::{registry_artifact, registry_dependency, skill_preferred_tool, tool};
use crate::modules::embodiment::sensors::SensorRegistry;
use crate::modules::memory::policy::{
    detect_sensitive_memory, memory_content_equivalent, memory_merge_key, merge_memory_patch,
    normalize_memory_layer,
};
use crate::modules::platform::lifecycle::{ModuleHealth, ModuleLifecycle};
use crate::registry::{ArtifactRef, ArtifactRegistry, KIND_TOOL, is_plugin_source};
use crate::tools::sensor::{SensorListTool, SensorReadTool};
use crate::{
    modules::memory::{MemoryListOpts, MemoryNode, RetrievalOpts},
    providers::adapter::{
        PreparedTool, ScheduleConfig, ScheduledTask, SchedulerBackend, ToolCall, ToolContext,
        ToolDefinition, ToolEntry, ToolRegistry, ToolResult, ToolRoutingEmbedding,
        ToolRoutingIndexEntry,
    },
};

const OBSOLETE_BUILTIN_TOOLS: &[&str] = &["device_control", "send_notification"];

struct ToolCatalogFields {
    routing_keywords: serde_json::Value,
    routing_examples: serde_json::Value,
    routing_text: String,
}

fn tool_catalog_fields(definition: &ToolDefinition) -> ToolCatalogFields {
    let schema_terms = schema_property_names(&definition.parameters).join(" ");
    let mut hints = tool_routing_hints(definition.name.as_str())
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
    entries: HashMap<String, Arc<ToolEntry>>,
    revisions: HashMap<String, String>,
}

impl std::ops::Deref for ExecutorCatalog {
    type Target = HashMap<String, Arc<ToolEntry>>;
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl ExecutorCatalog {
    fn insert(&mut self, name: String, executor: Arc<ToolEntry>) -> Option<Arc<ToolEntry>> {
        let revision = if self.revisions.contains_key(&name) {
            Uuid::new_v4().to_string()
        } else {
            "initial".into()
        };
        self.revisions.insert(name.clone(), revision);
        self.entries.insert(name, executor)
    }

    fn remove(&mut self, name: &str) -> Option<Arc<ToolEntry>> {
        // Retain the revision tombstone so remove + re-register cannot restore
        // the identity of an earlier approval in this process.
        self.entries.remove(name)
    }
}

pub struct BuiltinToolRegistry {
    /// Code-backed tools. Built-ins, Rust components, and in-process Rust
    /// plugins all live here; DB rows are only the discoverable catalog.
    executors: Arc<RwLock<ExecutorCatalog>>,
    /// DB connection for persisting plugin tools
    db: Option<DatabaseConnection>,
    /// Unified artifact/dependency graph manager.
    artifacts: Option<Arc<ArtifactRegistry>>,
    /// Memory backend used by memory_search and memory_store built-in tools.
    memory: Option<Arc<MemoryService>>,
    /// Scheduler backend used by schedule_task.
    scheduler: Option<Arc<dyn SchedulerBackend>>,
}

impl BuiltinToolRegistry {
    pub fn new() -> Self {
        Self {
            executors: Arc::default(),
            db: None,
            artifacts: None,
            memory: None,
            scheduler: None,
        }
    }

    pub fn with_runtime(
        db: DatabaseConnection,
        artifacts: Arc<ArtifactRegistry>,
        memory: Option<Arc<MemoryService>>,
        scheduler: Option<Arc<dyn SchedulerBackend>>,
    ) -> Self {
        Self {
            db: Some(db),
            artifacts: Some(artifacts),
            memory,
            scheduler,
            ..Self::new()
        }
    }

    /// Register sensor_read and sensor_list against the shared sensor plane.
    pub fn with_sensors(self, sensors: Arc<SensorRegistry>) -> Self {
        self.register_executor(
            SensorReadTool::new(sensors.clone())
                .entry()
                .expect("sensor_read schema"),
        );
        self.register_executor(
            SensorListTool::new(sensors)
                .entry()
                .expect("sensor_list schema"),
        );
        self
    }

    fn register_builtins(&self) -> anyhow::Result<()> {
        self.register_executor(GetCurrentTime.entry()?);
        #[cfg(feature = "tools-http")]
        self.register_executor(crate::tools::fetch::tool()?);
        self.register_executor(Calculate.entry()?);
        if let Some(memory) = &self.memory {
            self.register_executor(MemorySearch::new(Some(memory.clone())).entry()?);
            self.register_executor(MemoryStore::new(Some(memory.clone())).entry()?);
            self.register_executor(MemoryForget::new(Some(memory.clone())).entry()?);
        }
        if let Some(scheduler) = &self.scheduler {
            self.register_executor(ScheduleTask::new(scheduler.clone()).entry()?);
        }
        Ok(())
    }

    /// Register an additional tool executor at runtime (for tools that need external state).
    pub fn register_executor(&self, executor: Arc<ToolEntry>) {
        let name = executor.definition().name.to_string();
        self.executors.write().unwrap().insert(name, executor);
    }

    async fn persist_tool_definition(&self, definition: &ToolDefinition) -> anyhow::Result<()> {
        if let Some(db) = &self.db {
            let now = Utc::now();
            let artifact = ArtifactRef::tool(definition.name.to_string());
            let existing = tool::Entity::find()
                .filter(tool::Column::Name.eq(definition.name.as_str()))
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
                        name: Set(definition.name.to_string()),
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
        Ok(())
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
            .filter(|executor| executor.is_live())
            .map(|executor| executor.definition())
            .collect::<Vec<_>>();

        for definition in definitions {
            let artifact = ArtifactRef::tool(definition.name.to_string());
            let existing = tool::Entity::find()
                .filter(tool::Column::Name.eq(definition.name.as_str()))
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
                        name: Set(definition.name.to_string()),
                        description: Set(definition.description.clone()),
                        parameters: Set(definition.parameters.clone()),
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

    async fn init(&mut self) -> anyhow::Result<()> {
        self.remove_obsolete_builtin_tools()
            .await
            .context("removing obsolete built-in tools")?;
        self.register_builtins()?;

        self.upsert_executor_tools_to_db()
            .await
            .context("persisting built-in tool catalog")?;

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
        Ok(catalog
            .entries
            .get(name)
            .filter(|entry| entry.is_live())
            .map(|executor| PreparedTool {
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
            .filter(|executor| executor.is_live())
            .map(|executor| executor.definition())
            .collect();

        Ok(tools)
    }

    async fn list_tools_for(&self, user_id: Uuid) -> anyhow::Result<Vec<ToolDefinition>> {
        Ok(self
            .executors
            .read()
            .unwrap()
            .values()
            .filter(|entry| entry.is_live() && entry.allows_user(user_id))
            .map(|entry| entry.definition())
            .collect())
    }
    async fn get_tool(&self, name: &str) -> anyhow::Result<Option<ToolDefinition>> {
        // Check code-backed executors first
        if let Some(executor) = self
            .executors
            .read()
            .unwrap()
            .get(name)
            .filter(|entry| entry.is_live())
        {
            return Ok(Some(executor.definition()));
        }

        Ok(None)
    }

    async fn execute(&self, _call: ToolCall, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        anyhow::bail!("tools must execute through the durable Execution service")
    }

    async fn register_tool_executor(
        &self,
        owner_source: &str,
        executor: Arc<ToolEntry>,
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

        if let Some(existing) = self.executors.read().unwrap().get(definition.name.as_str()) {
            anyhow::ensure!(
                existing.definition().source == owner_source,
                "tool name is owned by another source"
            );
        }
        self.persist_tool_definition(&definition).await?;

        self.executors
            .write()
            .unwrap()
            .insert(definition.name.to_string(), executor);
        tracing::info!(
            tool = %definition.name,
            source = %definition.source,
            "registered code-backed tool executor"
        );
        Ok(())
    }

    async fn replace_source_tools(
        &self,
        owner: &str,
        entries: Vec<Arc<ToolEntry>>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            owner.starts_with("mcp:") || owner.starts_with("wasm:"),
            "managed source namespace required"
        );
        anyhow::ensure!(entries.len() <= 128, "tool catalog entry budget");
        let mut names = std::collections::HashSet::new();
        {
            let catalog = self.executors.read().unwrap();
            for entry in &entries {
                let definition = entry.definition();
                anyhow::ensure!(
                    definition.source == owner && names.insert(definition.name.to_string()),
                    "duplicate tool or source mismatch"
                );
                if let Some(existing) = catalog.get(definition.name.as_str()) {
                    anyhow::ensure!(
                        existing.definition().source == owner,
                        "tool name is owned by another source"
                    );
                }
            }
        }
        for entry in &entries {
            self.persist_tool_definition(&entry.definition()).await?;
        }
        let mut catalog = self.executors.write().unwrap();
        let old_names = catalog
            .values()
            .filter(|entry| entry.definition().source == owner)
            .map(|entry| entry.definition().name.to_string())
            .collect::<Vec<_>>();
        for name in old_names {
            catalog.remove(&name);
        }
        for entry in entries {
            catalog.insert(entry.definition().name.to_string(), entry);
        }
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

impl GetCurrentTime {
    pub fn entry(self) -> anyhow::Result<Arc<ToolEntry>> {
        let revision = "alia.builtin.GetCurrentTime";
        ToolEntry::from_tool(self, "builtin".to_string(), revision, false, None)
    }

    pub async fn execute(
        &self,
        arguments: GetCurrentTimeArgs,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let format = arguments.format.unwrap_or(TimeFormat::Human);
        let timezone = parse_time_tool_timezone(arguments.timezone.as_deref())?;

        let now = Utc::now();
        let local_now = now.with_timezone(&timezone);

        let time_str = match format {
            TimeFormat::Iso8601 => local_now.to_rfc3339(),
            TimeFormat::Unix => now.timestamp().to_string(),
            TimeFormat::Human => local_now.format("%Y-%m-%d %H:%M:%S %Z (%:z)").to_string(),
        };

        Ok(ToolResult {
            presentation: None,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetCurrentTimeArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<TimeFormat>,
}

impl rig_core::tool::Tool for GetCurrentTime {
    const NAME: &'static str = "get_current_time";
    type Args = GetCurrentTimeArgs;
    type Output = rig_core::tool::ToolOutput;
    type Error = rig_core::tool::ToolExecutionError;
    fn description(&self) -> String {
        "Get the current date and time. Optionally specify a timezone.".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        crate::tools::entry::parameters::<Self::Args>()
    }
    async fn call(
        &self,
        context: &mut rig_core::tool::ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let invocation = context
            .require::<ToolContext>()
            .map_err(crate::tools::entry::tool_error)?;
        self.execute(args, &invocation)
            .await
            .map_err(crate::tools::entry::tool_error)?
            .publish(context)
    }
}

fn parse_time_tool_timezone(value: Option<&str>) -> anyhow::Result<Tz> {
    let Some(raw) = value else {
        return Ok(chrono_tz::UTC);
    };
    let Some(timezone) = Some(raw.trim()).filter(|value| !value.is_empty()) else {
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

/// Evaluate a basic arithmetic expression.
pub struct Calculate;

impl Calculate {
    pub fn entry(self) -> anyhow::Result<Arc<ToolEntry>> {
        let revision = "alia.builtin.Calculate";
        ToolEntry::from_tool(self, "builtin".to_string(), revision, false, None)
    }

    pub async fn execute(
        &self,
        arguments: CalculateArgs,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let expression = arguments.expression.trim();
        anyhow::ensure!(!expression.is_empty(), "expression is required");
        let result = eval_arithmetic(expression)?;
        let display = format_number(result);

        Ok(ToolResult {
            presentation: None,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CalculateArgs {
    #[schemars(length(min = 1, max = 4096))]
    pub expression: String,
}

impl rig_core::tool::Tool for Calculate {
    const NAME: &'static str = "calculate";
    type Args = CalculateArgs;
    type Output = rig_core::tool::ToolOutput;
    type Error = rig_core::tool::ToolExecutionError;
    fn description(&self) -> String {
        "Evaluate a basic arithmetic expression using +, -, *, /, ^, and parentheses.".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        crate::tools::entry::parameters::<Self::Args>()
    }
    async fn call(
        &self,
        context: &mut rig_core::tool::ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let invocation = context
            .require::<ToolContext>()
            .map_err(crate::tools::entry::tool_error)?;
        self.execute(args, &invocation)
            .await
            .map_err(crate::tools::entry::tool_error)?
            .publish(context)
    }
}

/// Search user's memory
pub struct MemorySearch {
    memory: Option<Arc<MemoryService>>,
}

impl MemorySearch {
    pub fn new(memory: Option<Arc<MemoryService>>) -> Self {
        Self { memory }
    }
}

impl MemorySearch {
    pub fn entry(self) -> anyhow::Result<Arc<ToolEntry>> {
        let revision = "alia.builtin.MemorySearch";
        ToolEntry::from_tool(self, "builtin".to_string(), revision, false, None)
    }

    pub async fn execute(
        &self,
        arguments: MemorySearchArgs,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let query = arguments.query.trim();
        anyhow::ensure!(!query.is_empty(), "query is required");
        let Some(memory) = &self.memory else {
            return Ok(ToolResult {
                presentation: None,
                call_id: String::new(),
                success: false,
                output: serde_json::json!({"error": "memory_backend_unavailable"}),
                display: Some("Memory backend is not available".to_string()),
            });
        };

        if ctx.source == "ai" {
            anyhow::ensure!(
                ctx.context_id.is_some(),
                "AI memory search requires original context lineage"
            );
            // Execution checks the current catalog, confirmation and original
            // context before dispatch. The Pipeline then performs the read as
            // an extension of that context; no unversioned memory text is
            // copied into this execution receipt.
            return Ok(ToolResult {
                presentation: None,
                call_id: String::new(),
                success: true,
                output: serde_json::json!({"context_read_requested": true}),
                display: None,
            });
        }

        let max_results = arguments.max_results.unwrap_or(5);
        anyhow::ensure!(
            (1..=20).contains(&max_results),
            "max_results must be between 1 and 20"
        );
        let layers = arguments.layers.map(|layers| {
            layers
                .iter()
                .map(|layer| normalize_memory_layer(layer.trim()))
                .collect()
        });
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
            presentation: None,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemorySearchArgs {
    #[schemars(length(min = 1, max = 4096))]
    pub query: String,
    #[schemars(range(min = 1, max = 20))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layers: Option<Vec<String>>,
}

impl rig_core::tool::Tool for MemorySearch {
    const NAME: &'static str = "memory_search";
    type Args = MemorySearchArgs;
    type Output = rig_core::tool::ToolOutput;
    type Error = rig_core::tool::ToolExecutionError;
    fn description(&self) -> String {
        "Search the user's memory for relevant information.".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        crate::tools::entry::parameters::<Self::Args>()
    }
    async fn call(
        &self,
        context: &mut rig_core::tool::ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let invocation = context
            .require::<ToolContext>()
            .map_err(crate::tools::entry::tool_error)?;
        self.execute(args, &invocation)
            .await
            .map_err(crate::tools::entry::tool_error)?
            .publish(context)
    }
}

/// Store information to memory
pub struct MemoryStore {
    memory: Option<Arc<MemoryService>>,
}

impl MemoryStore {
    pub fn new(memory: Option<Arc<MemoryService>>) -> Self {
        Self { memory }
    }
}

impl MemoryStore {
    pub fn entry(self) -> anyhow::Result<Arc<ToolEntry>> {
        let revision = "alia.builtin.MemoryStore";
        ToolEntry::from_tool(self, "builtin".to_string(), revision, false, None)
    }

    pub async fn execute(
        &self,
        arguments: MemoryStoreArgs,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let content = arguments.content.trim().to_string();
        anyhow::ensure!(!content.is_empty(), "content is required");
        if let Some(sensitive) = detect_sensitive_memory(&content) {
            return Ok(ToolResult {
                presentation: None,
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
                presentation: None,
                call_id: String::new(),
                success: false,
                output: serde_json::json!({"error": "memory_backend_unavailable"}),
                display: Some("Memory backend is not available".to_string()),
            });
        };

        let layer = arguments
            .layer
            .unwrap_or(MemoryLayer::Episodic)
            .as_str()
            .to_owned();
        let importance = arguments.importance.unwrap_or(0.5);
        anyhow::ensure!(
            (0.0..=1.0).contains(&importance),
            "importance must be between zero and one"
        );
        let node_type = arguments
            .node_type
            .unwrap_or_else(|| "fact".into())
            .trim()
            .to_owned();
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
            presentation: None,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryStoreArgs {
    #[schemars(length(min = 1, max = 65536))]
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<MemoryLayer>,
    #[schemars(range(min = 0, max = 1))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub importance: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
}

impl rig_core::tool::Tool for MemoryStore {
    const NAME: &'static str = "memory_store";
    type Args = MemoryStoreArgs;
    type Output = rig_core::tool::ToolOutput;
    type Error = rig_core::tool::ToolExecutionError;
    fn description(&self) -> String {
        "Store information only when the user explicitly asks the assistant to remember it. Do not store secrets, identifiers, health details, religious views, or political views from chat; sensitive memories must be added by the user in the memory UI/API.".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        crate::tools::entry::parameters::<Self::Args>()
    }
    async fn call(
        &self,
        context: &mut rig_core::tool::ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let invocation = context
            .require::<ToolContext>()
            .map_err(crate::tools::entry::tool_error)?;
        self.execute(args, &invocation)
            .await
            .map_err(crate::tools::entry::tool_error)?
            .publish(context)
    }
}

/// Delete information from memory when the user explicitly asks to forget it.
pub struct MemoryForget {
    memory: Option<Arc<MemoryService>>,
}

impl MemoryForget {
    pub fn new(memory: Option<Arc<MemoryService>>) -> Self {
        Self { memory }
    }
}

impl MemoryForget {
    pub fn entry(self) -> anyhow::Result<Arc<ToolEntry>> {
        let revision = "alia.builtin.MemoryForget";
        ToolEntry::from_tool(self, "builtin".to_string(), revision, false, None)
    }

    pub async fn execute(
        &self,
        arguments: MemoryForgetArgs,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let Some(memory) = &self.memory else {
            return Ok(ToolResult {
                presentation: None,
                call_id: String::new(),
                success: false,
                output: serde_json::json!({"error": "memory_backend_unavailable"}),
                display: Some("Memory backend is not available".to_string()),
            });
        };

        let selection = arguments;
        anyhow::ensure!(
            selection.id.is_some() ^ selection.query.is_some(),
            "provide exactly one memory id or query"
        );
        let max_results = selection.max_results.unwrap_or(1);
        anyhow::ensure!(
            (1..=10).contains(&max_results),
            "max_results must be between 1 and 10"
        );
        anyhow::ensure!(
            ctx.source != "ai" || ctx.source_event_id.is_some(),
            "AI memory forget requires a persisted source user message"
        );
        // Capture before reading or embedding the selection. A later clear or
        // revision change must invalidate this request, not refresh its authority.
        let mut context = tool_memory_context(memory.as_ref(), ctx).await?;
        let matches = if let Some(id) = selection.id.as_deref() {
            anyhow::ensure!(!id.trim().is_empty(), "memory id must not be empty");
            let Some(node) = memory.get(ctx.user_id, id.trim()).await? else {
                return Ok(ToolResult {
                    presentation: None,
                    call_id: String::new(),
                    success: false,
                    output: serde_json::json!({"error": "memory_not_found", "id": id}),
                    display: Some("Memory not found".into()),
                });
            };
            vec![node]
        } else {
            let query = selection.query.as_deref().unwrap().trim();
            anyhow::ensure!(!query.is_empty(), "memory query must not be empty");
            let opts = RetrievalOpts {
                max_results: Some(max_results),
                min_similarity: None,
                layers: None,
            };
            let mut matches = memory.retrieve(ctx.user_id, query, opts.clone()).await?;
            if matches.is_empty() {
                matches =
                    retrieve_memory_by_terms(memory.as_ref(), ctx.user_id, query, &opts).await?;
            }
            matches
        };
        let targets: Vec<_> = matches
            .into_iter()
            .take(max_results as usize)
            .filter_map(|node| node.id.map(|id| (id, node.revision)))
            .collect();
        context.source_memories = targets.clone();
        let deleted_count = memory.forget_many_guarded(&context, &targets).await?;
        let ids: Vec<_> = targets.iter().map(|(id, _)| id.as_str()).collect();
        Ok(ToolResult {
            presentation: None,
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "deleted": deleted_count, "selected": ids.len(), "ids": ids,
                "query": selection.query,
            }),
            display: Some(
                if deleted_count == 0 {
                    "No matching memories found"
                } else {
                    "Memory forgotten"
                }
                .into(),
            ),
        })
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryForgetArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[schemars(range(min = 1, max = 10))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
}

impl rig_core::tool::Tool for MemoryForget {
    const NAME: &'static str = "memory_forget";
    type Args = MemoryForgetArgs;
    type Output = rig_core::tool::ToolOutput;
    type Error = rig_core::tool::ToolExecutionError;
    fn description(&self) -> String {
        "Forget/delete saved user memories only when the user explicitly asks to forget something. Provide either an exact memory id, or a text query describing the memory to forget.".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        crate::tools::entry::parameters::<Self::Args>()
    }
    async fn call(
        &self,
        context: &mut rig_core::tool::ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let invocation = context
            .require::<ToolContext>()
            .map_err(crate::tools::entry::tool_error)?;
        self.execute(args, &invocation)
            .await
            .map_err(crate::tools::entry::tool_error)?
            .publish(context)
    }
}

async fn upsert_tool_memory(
    memory: &MemoryService,
    ctx: &ToolContext,
    incoming: &MemoryNode,
) -> anyhow::Result<&'static str> {
    anyhow::ensure!(
        ctx.source != "ai" || ctx.source_event_id.is_some(),
        "AI memory writes require a persisted source user message"
    );
    let context = tool_memory_context(memory, ctx).await?;
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

async fn tool_memory_context(
    memory: &MemoryService,
    ctx: &ToolContext,
) -> anyhow::Result<crate::modules::memory::MemoryWriteContext> {
    let source_epoch = ctx
        .source_privacy_epoch
        .context("memory tool requires a trusted source privacy epoch")?;
    let mut context = memory.write_snapshot(ctx.user_id).await?;
    anyhow::ensure!(
        source_epoch >= 0 && context.privacy_epoch == source_epoch,
        "memory source privacy epoch changed"
    );
    context.source_received_at = ctx.source_received_at;
    context.source_event_ids = ctx.source_event_id.into_iter().collect();
    context.exact_source_received_at = ctx.source_event_id.map(|id| (id, ctx.source_received_at));
    if ctx.source == "ai" {
        let id = ctx
            .context_id
            .context("AI memory tool requires original context lineage")?;
        context = memory
            .bind_context(context, id, crate::modules::memory::MemoryWriteOrigin::Ai)
            .await?;
        anyhow::ensure!(
            context.source_received_at.timestamp_micros()
                == ctx.source_received_at.timestamp_micros()
                && context
                    .exact_source_received_at
                    .map(|(id, at)| (id, at.timestamp_micros()))
                    == ctx
                        .source_event_id
                        .map(|id| (id, ctx.source_received_at.timestamp_micros())),
            "AI memory tool source differs from its original context"
        );
    } else if ctx.source == "scheduler" {
        context.origin = crate::modules::memory::MemoryWriteOrigin::Maintenance;
    }
    // Even a duplicate/no-op result must validate its actual source. This read
    // check does not replace guarded commit's atomic validation after selection.
    memory.validate_write_context(&context).await?;
    Ok(context)
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

impl ScheduleTask {
    pub fn entry(self) -> anyhow::Result<Arc<ToolEntry>> {
        let revision = "alia.builtin.ScheduleTask";
        ToolEntry::from_tool(self, "builtin".to_string(), revision, true, None)
    }

    pub async fn execute(
        &self,
        arguments: ScheduleTaskArgs,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let name = arguments.name.trim().to_owned();
        anyhow::ensure!(!name.is_empty(), "name is required");
        let task = ScheduledTask {
            id: Uuid::new_v4(),
            user_id: ctx.user_id,
            name: name.to_string(),
            enabled: true,
            schedule: ScheduleConfig {
                schedule_type: arguments.schedule_type.as_str().to_owned(),
                cron_expression: arguments.cron_expression,
                run_at: arguments.run_at,
                every_sec: arguments.every_sec,
                poll_interval_sec: arguments.poll_interval_sec,
            },
            conditions: arguments
                .conditions
                .unwrap_or_else(|| serde_json::json!([])),
            action: serde_json::Value::Object(arguments.action),
            cooldown_sec: arguments.cooldown_sec.unwrap_or(0),
            max_retries: arguments.max_retries.unwrap_or(0),
            created_by: ctx.source.clone(),
        };
        let task_id = self.scheduler.create_task(task.clone()).await?;

        Ok(ToolResult {
            presentation: None,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleTaskArgs {
    #[schemars(length(min = 1, max = 256))]
    pub name: String,
    pub schedule_type: ScheduleType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cron_expression: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every_sec: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_interval_sec: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<serde_json::Value>,
    pub action: serde_json::Map<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_sec: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
}

impl rig_core::tool::Tool for ScheduleTask {
    const NAME: &'static str = "schedule_task";
    type Args = ScheduleTaskArgs;
    type Output = rig_core::tool::ToolOutput;
    type Error = rig_core::tool::ToolExecutionError;
    fn description(&self) -> String {
        "Create a scheduled task to run at a specific time or on a recurring schedule.".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        crate::tools::entry::parameters::<Self::Args>()
    }
    async fn call(
        &self,
        context: &mut rig_core::tool::ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let invocation = context
            .require::<ToolContext>()
            .map_err(crate::tools::entry::tool_error)?;
        self.execute(args, &invocation)
            .await
            .map_err(crate::tools::entry::tool_error)?
            .publish(context)
    }
}

async fn retrieve_memory_by_terms(
    memory: &MemoryService,
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
            function: rig_core::completion::ToolDefinition::new(
                rig_core::message::ToolName::new("memory_store".to_string()).unwrap(),
                "Store information to memory".to_string(),
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "content": {"type": "string"}
                    }
                }),
            ),
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
            name: definition.name.to_string(),
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
                name: definition.name.to_string(),
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TimeFormat {
    Iso8601,
    Human,
    Unix,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemoryLayer {
    Core,
    Preference,
    Relationship,
    Episodic,
    Procedural,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleType {
    Cron,
    Once,
    Interval,
    Condition,
}

impl MemoryLayer {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Preference => "preference",
            Self::Relationship => "relationship",
            Self::Episodic => "episodic",
            Self::Procedural => "procedural",
        }
    }
}
impl ScheduleType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Cron => "cron",
            Self::Once => "once",
            Self::Interval => "interval",
            Self::Condition => "condition",
        }
    }
}
