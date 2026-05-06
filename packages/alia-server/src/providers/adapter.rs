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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMatch {
    pub name: String,
    pub confidence: f32,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillResult {
    pub success: bool,
    pub output: serde_json::Value,
    pub message: Option<String>,
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
    async fn match_skill(&self, intent: &str) -> anyhow::Result<Option<SkillMatch>>;
    async fn execute(
        &self,
        skill: &SkillMatch,
        params: serde_json::Value,
    ) -> anyhow::Result<SkillResult>;
    async fn list_skills(&self) -> anyhow::Result<Vec<SkillInfo>>;
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
    async fn render_system_prompt(
        &self,
        user_id: Uuid,
        vars: PromptVars,
    ) -> anyhow::Result<String>;
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
    CacheHit { response: String },
    Skill { skill_name: String, params: serde_json::Value },
    LocalLlm,
    RemoteLlm,
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
