use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::module::ModuleLifecycle;

// ========== Common types ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioEncoding {
    Pcm16kMono,
    Opus,
    Wav,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscribeChunk {
    pub text: String,
    pub is_final: bool,
}

// ========== LLM Adapter ==========

#[async_trait]
pub trait LlmAdapter: ModuleLifecycle {
    /// Single-turn chat (non-streaming)
    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String>;

    /// Streaming chat, returns a token stream
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>>;
}

// ========== STT Adapter ==========

#[async_trait]
pub trait SttAdapter: ModuleLifecycle {
    /// Transcribe complete audio buffer
    async fn transcribe(&self, audio: &[u8], encoding: AudioEncoding) -> anyhow::Result<String>;

    /// Streaming transcription
    async fn transcribe_stream(
        &self,
        audio_stream: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
        encoding: AudioEncoding,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<TranscribeChunk>> + Send>>>;
}

// ========== TTS Adapter ==========

#[async_trait]
pub trait TtsAdapter: ModuleLifecycle {
    /// Synthesize complete audio
    async fn synthesize(&self, text: &str) -> anyhow::Result<Vec<u8>>;

    /// Streaming synthesis
    async fn synthesize_stream(
        &self,
        text: &str,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<Vec<u8>>> + Send>>>;
}

// ========== Embedding Adapter ==========

#[async_trait]
pub trait EmbeddingAdapter: ModuleLifecycle {
    /// Batch text embedding
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>>;
    /// Vector dimension
    fn dimension(&self) -> usize;
}

// ========== Memory Backend ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryNode {
    pub id: Option<Uuid>,
    pub memory_layer: String,
    pub node_type: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub importance: f32,
}

#[derive(Debug, Clone, Default)]
pub struct RetrievalOpts {
    pub max_results: Option<u32>,
    pub min_similarity: Option<f32>,
    pub layers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPatch {
    pub content: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub importance: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphFragment {
    pub nodes: Vec<MemoryNode>,
    pub edges: Vec<MemoryEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEdge {
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub relation_type: String,
    pub weight: f32,
}

#[async_trait]
pub trait MemoryBackend: ModuleLifecycle {
    async fn store(&self, user_id: Uuid, node: MemoryNode) -> anyhow::Result<Uuid>;
    async fn retrieve(
        &self,
        user_id: Uuid,
        query: &str,
        opts: RetrievalOpts,
    ) -> anyhow::Result<Vec<MemoryNode>>;
    async fn update(&self, node_id: Uuid, patch: MemoryPatch) -> anyhow::Result<()>;
    async fn delete(&self, node_id: Uuid) -> anyhow::Result<()>;
    async fn graph_neighbors(&self, node_id: Uuid, depth: u32) -> anyhow::Result<GraphFragment>;
    async fn refresh_communities(&self, _user_id: Uuid) -> anyhow::Result<()> {
        Ok(())
    }
}

// ========== Cache Backend ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedResponse {
    pub response_text: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub user_id: Option<Uuid>,
    pub scope: String,
    pub query_text: String,
    pub response_text: String,
    pub metadata: serde_json::Value,
    pub ttl_seconds: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CacheFilter {
    pub user_id: Option<Uuid>,
    pub scope: Option<String>,
}

#[async_trait]
pub trait CacheBackend: ModuleLifecycle {
    async fn get(
        &self,
        user_id: Option<Uuid>,
        query: &str,
    ) -> anyhow::Result<Option<CachedResponse>>;
    async fn put(&self, entry: CacheEntry) -> anyhow::Result<()>;
    async fn invalidate(&self, filter: CacheFilter) -> anyhow::Result<u64>;
}

// ========== Skill Provider ==========
//
// Skills extend what the LLM **knows how to do well**.
// They provide domain knowledge (prompt templates) and specify which tools
// are relevant. Skill execution goes through the LLM with tool access.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMatch {
    pub name: String,
    pub confidence: f32,
    pub parameters: serde_json::Value,
}

/// Resolved skill context for LLM-driven execution.
///
/// When a skill is triggered, the pipeline sends this to the LLM:
/// - `system_prompt` provides domain-specific instructions
/// - `tools` lists which tools should be made available for function calling
/// - The LLM decides which tools to call (if any)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillResolution {
    pub skill_name: String,
    /// Domain-specific system prompt (from the skill's prompt_template)
    pub system_prompt: Option<String>,
    /// Tool names to make available to the LLM during this skill execution.
    /// Empty means all tools are available.
    pub tools: Vec<String>,
    /// Whether the LLM response can be cached
    pub cacheable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub category: String,
}

#[async_trait]
pub trait SkillProvider: ModuleLifecycle {
    /// Match user intent to a skill.
    async fn match_skill(&self, intent: &str) -> anyhow::Result<Option<SkillMatch>>;
    /// Resolve a matched skill into LLM-facing context (prompt + available tools).
    async fn resolve(&self, skill: &SkillMatch) -> anyhow::Result<SkillResolution>;
    /// List all available skills.
    async fn list_skills(&self) -> anyhow::Result<Vec<SkillInfo>>;
    /// Register a skill from a plugin. Persisted to DB with plugin name as source.
    async fn register_plugin_skill(
        &self,
        plugin_name: &str,
        skill: PluginSkillDefinition,
    ) -> anyhow::Result<()>;
    /// Unregister all skills provided by a plugin.
    async fn unregister_plugin_skills(&self, plugin_name: &str) -> anyhow::Result<u64>;
}

/// Skill definition provided by a plugin at runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSkillDefinition {
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub category: String,
    pub intent_patterns: Vec<String>,
    pub parameters: serde_json::Value,
    pub tools: Vec<String>,
    pub prompt_template: Option<String>,
    #[serde(default)]
    pub cacheable: bool,
    #[serde(default)]
    pub priority: i32,
}

// ========== Persona Provider ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persona {
    pub name: String,
    pub system_prompt_template: String,
    pub personality_traits: serde_json::Value,
    pub voice_config: serde_json::Value,
    pub greeting: Option<String>,
    pub behavior_guidelines: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PromptVars {
    pub core_memories: Vec<String>,
    pub user_preferences: Vec<(String, String)>,
    pub current_time: String,
    pub user_presence: String,
    pub device_status_summary: String,
    pub available_skills: Vec<String>,
}

#[async_trait]
pub trait PersonaProvider: ModuleLifecycle {
    async fn get_effective_persona(&self, user_id: Uuid) -> anyhow::Result<Persona>;
    async fn render_system_prompt(&self, user_id: Uuid, vars: PromptVars)
    -> anyhow::Result<String>;
}

// ========== Conversation Backend ==========

#[async_trait]
pub trait ConversationBackend: ModuleLifecycle {
    /// Get the active session ID for a channel/user combo, creating one if none exists.
    async fn get_or_create_active_session(
        &self,
        channel_id: &str,
        user_id: &str,
    ) -> anyhow::Result<Uuid>;

    /// Create a new session explicitly and return its ID.
    async fn create_session(&self, channel_id: &str, user_id: &str) -> anyhow::Result<Uuid>;

    /// List all sessions for a channel/user, ordered by recency.
    async fn list_sessions(
        &self,
        channel_id: &str,
        user_id: &str,
    ) -> anyhow::Result<Vec<(Uuid, Option<String>, chrono::DateTime<chrono::Utc>)>>;

    /// Set the active session for a channel/user.
    async fn set_active_session(
        &self,
        channel_id: &str,
        user_id: &str,
        session_id: Uuid,
    ) -> anyhow::Result<()>;

    /// Fetch the last N messages from a session.
    async fn get_history(&self, session_id: Uuid, limit: u64) -> anyhow::Result<Vec<ChatMessage>>;

    /// Append a message to a session.
    async fn append_message(&self, session_id: Uuid, message: ChatMessage) -> anyhow::Result<()>;
}

// ========== Meditation Strategy ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeditationTask {
    pub user_id: Uuid,
    pub phase: String,
    pub trigger: String,
    pub context: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeditationStatus {
    pub enabled: bool,
    pub last_phase: Option<String>,
    pub last_executed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub pending_tasks: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeditationPhase {
    Extraction,
    Consolidation,
    Reflection,
    HabitDetection,
}

#[async_trait]
pub trait MeditationStrategy: ModuleLifecycle {
    async fn schedule(&self, task: MeditationTask) -> anyhow::Result<()>;
    async fn status(&self, user_id: Uuid) -> anyhow::Result<MeditationStatus>;
    async fn trigger(&self, user_id: Uuid, phase: MeditationPhase) -> anyhow::Result<()>;
    /// Get the current meditation trigger conditions (config defaults merged with user overrides)
    async fn get_conditions(&self) -> Vec<serde_json::Value>;
    /// Set user-level condition overrides (replaces config defaults at runtime)
    async fn set_conditions(&self, conditions: Vec<serde_json::Value>) -> anyhow::Result<()>;
}

// ========== Router Strategy ==========

#[derive(Debug, Clone)]
pub struct RouterInput {
    pub text: String,
    pub user_id: Uuid,
    pub conversation_context: Vec<ChatMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteDecision {
    CacheHit {
        response: String,
    },
    Skill {
        skill_name: String,
        params: serde_json::Value,
    },
    LocalLlm {
        system_prompt: Option<String>,
    },
    RemoteLlm {
        system_prompt: Option<String>,
    },
}

#[async_trait]
pub trait RouterStrategy: ModuleLifecycle {
    async fn route(&self, input: &RouterInput) -> anyhow::Result<RouteDecision>;
}

// ========== Rule Evaluator ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleAction {
    pub rule_id: Uuid,
    pub rule_name: String,
    pub action_type: String,
    pub payload: serde_json::Value,
}

#[async_trait]
pub trait RuleEvaluator: ModuleLifecycle {
    async fn evaluate(&self, event: &crate::event::ServerEvent) -> anyhow::Result<Vec<RuleAction>>;
    async fn reload(&self) -> anyhow::Result<()>;
}

// ========== Scheduler Backend ==========

/// Scheduled task trigger types: cron, once, or interval
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleConfig {
    #[serde(rename = "type")]
    pub schedule_type: String, // "cron" | "once" | "interval"
    #[serde(default)]
    pub cron_expression: Option<String>, // For cron type
    #[serde(default)]
    pub run_at: Option<String>, // For once type (ISO 8601)
    #[serde(default)]
    pub every_sec: Option<u32>, // For interval type
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub schedule: ScheduleConfig,
    pub conditions: serde_json::Value, // JSON array of condition configs
    pub action: serde_json::Value,     // Action config (type + payload)
    pub cooldown_sec: u32,
    pub max_retries: u32,
    pub created_by: String, // "user" | "ai"
}

#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub user_id: Option<Uuid>,
    pub enabled_only: bool,
}

#[derive(Debug, Clone)]
pub struct TaskPatch {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub schedule: Option<ScheduleConfig>,
    pub conditions: Option<serde_json::Value>,
    pub action: Option<serde_json::Value>,
}

/// Event subscription: trigger actions when EventBus events match a pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSubscription {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub event_pattern: serde_json::Value, // Pattern to match against ServerEvent
    pub conditions: serde_json::Value,    // Additional conditions
    pub action: serde_json::Value,        // Action config
    pub cooldown_sec: u32,
    pub created_by: String,
}

/// Trait for plugin-provided condition evaluators
#[async_trait]
pub trait ConditionEvaluator: Send + Sync {
    fn condition_type(&self) -> &str;
    async fn evaluate(&self, config: &serde_json::Value) -> anyhow::Result<bool>;
}

/// Trait for plugin-provided action executors
#[async_trait]
pub trait ActionExecutor: Send + Sync {
    fn action_type(&self) -> &str;
    async fn execute(&self, config: &serde_json::Value) -> anyhow::Result<serde_json::Value>;
}

#[async_trait]
pub trait SchedulerBackend: ModuleLifecycle {
    /// Create a new scheduled task
    async fn create_task(&self, task: ScheduledTask) -> anyhow::Result<Uuid>;
    /// Update a task
    async fn update_task(&self, task_id: Uuid, patch: TaskPatch) -> anyhow::Result<()>;
    /// Delete a task
    async fn delete_task(&self, task_id: Uuid) -> anyhow::Result<()>;
    /// List tasks with filters
    async fn list_tasks(&self, filter: TaskFilter) -> anyhow::Result<Vec<ScheduledTask>>;
    /// Pause a task
    async fn pause_task(&self, task_id: Uuid) -> anyhow::Result<()>;
    /// Resume a task
    async fn resume_task(&self, task_id: Uuid) -> anyhow::Result<()>;
    /// Subscribe to events
    async fn subscribe_event(&self, sub: EventSubscription) -> anyhow::Result<Uuid>;
    /// Unsubscribe from events
    async fn unsubscribe_event(&self, sub_id: Uuid) -> anyhow::Result<()>;
    /// List event subscriptions
    async fn list_subscriptions(&self, user_id: Uuid) -> anyhow::Result<Vec<EventSubscription>>;
}

// ========== Tool Registry ==========

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema
    pub source: String,                // "builtin" | plugin name
    pub require_confirmation: bool,
    pub required_role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub call_id: String, // LLM-assigned ID for result correlation
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub success: bool,
    pub output: serde_json::Value,
    pub display: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub user_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub source: String, // "ai" | "manual" | "scheduler"
}

#[async_trait]
pub trait ToolRegistry: ModuleLifecycle {
    /// List all available tools
    async fn list_tools(&self) -> anyhow::Result<Vec<ToolDefinition>>;
    /// Get a specific tool definition
    async fn get_tool(&self, name: &str) -> anyhow::Result<Option<ToolDefinition>>;
    /// Execute a tool call
    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> anyhow::Result<ToolResult>;
    /// Register a plugin tool
    async fn register_plugin_tool(&self, tool: ToolDefinition) -> anyhow::Result<()>;
    /// Unregister a plugin tool
    async fn unregister_plugin_tool(&self, tool_name: &str) -> anyhow::Result<()>;
}
