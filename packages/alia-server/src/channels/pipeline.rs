//! Channel → AI Pipeline integration.
//!
//! Subscribes to `ServerEvent::ChannelInput` on the EventBus, routes each
//! inbound `ChannelEvent` through: Router → Cache/Skill/LLM → ChannelResponse,
//! then delivers the response back to the originating channel.
//!
//! When the originating channel has `capabilities().streaming == true` and the
//! route decision is an LLM call, the pipeline uses `chat_stream` and delivers
//! tokens incrementally via `begin_stream / send_stream_chunk / end_stream`.
//! For non-streaming channels the pipeline calls `send_message` with the full
//! response.

use std::sync::Arc;

use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::channels::ChannelRegistry;
use crate::channels::adapter::{
    AudioFormat, AudioPayload, ChannelAdapter, ChannelEvent, ChannelTarget,
};
use crate::config::{
    CachePolicyResourceSection, CacheSection, ConversationSection, MemoryCaptureMode,
    MemorySection, ToolsSection,
};
use crate::event::{EventBus, ServerEvent};
use crate::memory_policy::{detect_sensitive_memory, memory_prompt_line, normalize_memory_layers};
use crate::module::ModuleRegistry;
use crate::providers::adapter::{
    AudioEncoding, CACHE_METADATA_SCHEMA, CacheEntry, CachedResponse, ChatMessage,
    ConversationEventEntry, DEFAULT_ASSISTANT_NAME, DEFAULT_CACHE_SCOPE, LlmAdapter,
    MemoryContextPack, RetrievalOpts, RouteDecision, RouterInput, SemanticCacheCandidate,
    ToolDefinition, TtsRequest, sanitize_assistant_name,
};
use crate::providers::sensor::SensorRegistry;
use crate::settings::values::{
    bool_setting, f32_setting, string_setting, u32_setting, u64_setting, usize_setting,
};

mod debug;
mod dialogue_policy;
mod error_notice;
#[cfg(test)]
mod integration_tests;
pub mod memory_capture;
pub mod streaming;
mod supervision;
pub mod tools;
mod transcript_correction;
mod turn;
mod voice;
use crate::expression::orchestrator::{
    ExpressionExecutionPhase, cancel_expression_execution, execute_expression_plan,
    execute_expression_plan_phase, negotiate_expression_capabilities,
};
use crate::expression::{
    AffectAssistantResponse, AffectUserInput, ExpressionDeviceContext, ExpressionInput,
    ExpressionPlan, ExpressionSafetyContext, ExpressionUserState,
};
use crate::modules::interaction::voice_input::RecognizedVoiceInput;
use debug::*;
use dialogue_policy::DialoguePolicyInput;
use error_notice::{PipelineErrorStage, notify_pipeline_error};
use memory_capture::{MemoryCaptureTrace, capture_turn_memory};
use streaming::*;
use tools::*;
use transcript_correction::{
    TranscriptCorrectionContext, TranscriptCorrectionResult, correct_stt_transcript,
};
use turn::*;

const EMPTY_RESPONSE_FALLBACK: &str = "抱歉，我刚才没有生成有效回复，请再试一次。";
const CACHE_STATUS_ACTIVE: &str = "active";
const CACHE_STATUS_PENDING_INSIGHTS_REVIEW: &str = "pending_insights_review";

/// Result of `process_text` — carries the full response text and whether it
/// was already delivered to the channel via streaming.
struct TextResult {
    text: String,
    /// `true` when tokens were already delivered via `begin_stream / send_stream_chunk
    /// / end_stream`.  Callers must NOT call `send_message` again.
    streamed: bool,
    /// `true` when voice audio was already streamed while text was generated.
    /// Callers must NOT synthesize and send the same answer again.
    audio_streamed: bool,
    /// `true` when processing stopped because the active voice turn was interrupted.
    interrupted: bool,
    /// Persist this interaction after downstream delivery if the caller deferred
    /// history/cache writes to avoid storing interrupted assistant output.
    pending_persistence: Option<PendingTextPersistence>,
    expression_plan: Option<ExpressionPlan>,
    expression_input: Option<ExpressionInput>,
    tts_trace: Option<TtsTraceContext>,
}

impl TextResult {
    fn interrupted() -> Self {
        Self {
            text: String::new(),
            streamed: false,
            audio_streamed: false,
            interrupted: true,
            pending_persistence: None,
            expression_plan: None,
            expression_input: None,
            tts_trace: None,
        }
    }
}

struct PlannedExpression {
    plan: ExpressionPlan,
    input: ExpressionInput,
}

#[derive(Debug, Clone)]
pub struct PendingTextPersistence {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub user_id_text: String,
    pub query_text: String,
    pub response_text: String,
    pub channel_id: String,
    pub cache_write: Option<CacheWrite>,
}

#[derive(Debug, Clone)]
struct InterruptedUserContext {
    session_id: Uuid,
    turn_id: Uuid,
    user_id: Uuid,
    user_id_text: String,
    query_text: String,
    channel_id: String,
    reason: String,
    stage: &'static str,
}

#[derive(Debug, Clone)]
pub struct CacheWrite {
    pub scope: String,
    pub metadata: serde_json::Value,
    pub ttl_seconds: u64,
    pub store_embedding: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CacheContext {
    pub memory_context_used: bool,
    pub used_tools: bool,
    pub skill_cacheable: Option<bool>,
    pub response_was_fallback: bool,
    pub route: &'static str,
}

#[derive(Debug, Clone)]
struct LlmExecutionOutcome {
    text: String,
    streamed: bool,
    audio_streamed: bool,
    used_tools: bool,
}

#[derive(Debug, Clone)]
pub(super) struct AgentTrace {
    session_id: Uuid,
    turn_id: Uuid,
    next_sequence: i32,
}

impl AgentTrace {
    fn new(session_id: Uuid) -> Self {
        Self {
            session_id,
            turn_id: Uuid::new_v4(),
            next_sequence: 0,
        }
    }

    pub(super) async fn record(
        &mut self,
        modules: &ModuleRegistry,
        event_type: impl Into<String>,
        actor: impl Into<String>,
        summary: Option<String>,
        payload: serde_json::Value,
    ) {
        let event = ConversationEventEntry {
            turn_id: self.turn_id,
            sequence: self.next_sequence,
            event_type: event_type.into(),
            actor: actor.into(),
            summary,
            payload,
        };
        self.next_sequence += 1;
        if let Err(error) = modules
            .conversation
            .append_event(self.session_id, event)
            .await
        {
            tracing::debug!(error = ?error, "failed to append conversation event");
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimePromptContext {
    pub assistant_name: String,
    pub core_memories: Vec<String>,
    pub memory_context: Option<MemoryContextPack>,
    pub relationship_summary: Option<String>,
    pub relationship_preference_summary: Option<String>,
    pub user_preferences: Vec<(String, String)>,
    pub current_time: String,
    pub user_presence: String,
    pub device_status_summary: String,
    pub available_skills: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PipelinePolicies {
    pub conversation: ConversationSection,
    pub tools: ToolsSection,
    pub cache: CacheSection,
    pub cache_policy: CachePolicyResourceSection,
    pub memory: MemorySection,
    pub transcript_correction: crate::config::TranscriptCorrectionPolicySection,
}

pub type SharedPipelinePolicies = Arc<RwLock<PipelinePolicies>>;

fn parse_memory_capture_mode(value: &str) -> MemoryCaptureMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => MemoryCaptureMode::None,
        "explicit_tool_only" | "explicit-tool-only" => MemoryCaptureMode::ExplicitToolOnly,
        _ => MemoryCaptureMode::LlmSelected,
    }
}

impl PipelinePolicies {
    pub fn new(
        conversation: ConversationSection,
        tools: ToolsSection,
        cache: CacheSection,
        cache_policy: CachePolicyResourceSection,
        memory: MemorySection,
        transcript_correction: crate::config::TranscriptCorrectionPolicySection,
    ) -> Self {
        Self {
            conversation,
            tools,
            cache,
            cache_policy,
            memory,
            transcript_correction,
        }
    }

    pub fn apply_settings(
        &mut self,
        updates: &std::collections::BTreeMap<String, Value>,
    ) -> Vec<String> {
        let mut applied = Vec::new();

        if let Some(value) =
            usize_setting(updates, "core.policies.conversation.max_context_messages")
        {
            self.conversation.max_context_messages = value;
            applied.push("core.policies.conversation.max_context_messages".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.conversation.streaming_enabled") {
            self.conversation.streaming_enabled = value;
            applied.push("core.policies.conversation.streaming_enabled".to_string());
        }

        if let Some(value) = bool_setting(updates, "core.policies.cache.enabled") {
            self.cache.enabled = value;
            applied.push("core.policies.cache.enabled".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.cache.exact_match_enabled") {
            self.cache.exact_match_enabled = value;
            applied.push("core.policies.cache.exact_match_enabled".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.cache.semantic_match_enabled") {
            self.cache.semantic_match_enabled = value;
            applied.push("core.policies.cache.semantic_match_enabled".to_string());
        }
        if let Some(value) = f32_setting(updates, "core.policies.cache.semantic_threshold") {
            self.cache.semantic_threshold = value.clamp(0.0, 1.0);
            applied.push("core.policies.cache.semantic_threshold".to_string());
        }
        if let Some(value) = u64_setting(updates, "core.policies.cache.default_ttl_seconds") {
            self.cache.default_ttl_seconds = value;
            applied.push("core.policies.cache.default_ttl_seconds".to_string());
        }
        if let Some(value) = u64_setting(updates, "core.policies.cache.skill_cache_ttl_seconds") {
            self.cache.skill_cache_ttl_seconds = value;
            applied.push("core.policies.cache.skill_cache_ttl_seconds".to_string());
        }

        if let Some(value) = bool_setting(updates, "core.policies.memory.enabled") {
            self.memory.enabled = value;
            applied.push("core.policies.memory.enabled".to_string());
        }
        if let Some(value) = string_setting(updates, "core.policies.memory.capture_mode") {
            self.memory.capture_mode = parse_memory_capture_mode(&value);
            applied.push("core.policies.memory.capture_mode".to_string());
        }
        if let Some(value) = u64_setting(updates, "core.policies.memory.capture_timeout_sec") {
            self.memory.capture_timeout_sec = value.max(1);
            applied.push("core.policies.memory.capture_timeout_sec".to_string());
        }
        if let Some(value) = f32_setting(updates, "core.policies.memory.capture_min_confidence") {
            self.memory.capture_min_confidence = value.clamp(0.0, 1.0);
            applied.push("core.policies.memory.capture_min_confidence".to_string());
        }
        if let Some(value) = f32_setting(updates, "core.policies.memory.capture_min_importance") {
            self.memory.capture_min_importance = value.clamp(0.0, 1.0);
            applied.push("core.policies.memory.capture_min_importance".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.memory.sensitive_filter_enabled")
        {
            self.memory.sensitive_filter_enabled = value;
            applied.push("core.policies.memory.sensitive_filter_enabled".to_string());
        }
        if let Some(value) =
            string_setting(updates, "core.policies.memory.active_embedding_profile")
        {
            self.memory.active_embedding_profile = value;
            applied.push("core.policies.memory.active_embedding_profile".to_string());
        }
        if let Some(value) = u32_setting(updates, "core.policies.memory.graph_depth") {
            self.memory.graph_depth = value.clamp(1, 4);
            applied.push("core.policies.memory.graph_depth".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.memory.consolidation_enabled") {
            self.memory.consolidation_enabled = value;
            applied.push("core.policies.memory.consolidation_enabled".to_string());
        }
        if let Some(value) = updates.get("core.policies.memory.paused_until") {
            self.memory.paused_until = value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            applied.push("core.policies.memory.paused_until".to_string());
        }
        if let Some(value) = updates
            .get("core.policies.memory.retrieval_layers")
            .and_then(|value| value.as_array())
        {
            self.memory.retrieval_layers = value
                .iter()
                .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                .collect();
            applied.push("core.policies.memory.retrieval_layers".to_string());
        }
        if let Some(value) =
            usize_setting(updates, "core.policies.memory.max_injected_memory_chars")
        {
            self.memory.max_injected_memory_chars = value.clamp(256, 12_000);
            applied.push("core.policies.memory.max_injected_memory_chars".to_string());
        }
        if let Some(value) = u32_setting(updates, "core.policies.memory.max_retrieval_results") {
            self.memory.max_retrieval_results = value;
            applied.push("core.policies.memory.max_retrieval_results".to_string());
        }
        if let Some(value) = f32_setting(updates, "core.policies.memory.similarity_threshold") {
            self.memory.similarity_threshold = value.clamp(0.0, 1.0);
            applied.push("core.policies.memory.similarity_threshold".to_string());
        }

        if let Some(value) = u32_setting(updates, "core.policies.tools.max_tool_call_depth") {
            self.tools.max_tool_call_depth = value;
            applied.push("core.policies.tools.max_tool_call_depth".to_string());
        }
        if let Some(value) = u64_setting(updates, "core.policies.tools.tool_call_timeout_sec") {
            self.tools.tool_call_timeout_sec = value;
            applied.push("core.policies.tools.tool_call_timeout_sec".to_string());
        }
        if let Some(value) = usize_setting(updates, "core.policies.tools.max_tools_per_turn") {
            self.tools.max_tools_per_turn = value;
            applied.push("core.policies.tools.max_tools_per_turn".to_string());
        }
        if let Some(value) = usize_setting(updates, "core.policies.tools.candidate_pool_size") {
            self.tools.candidate_pool_size = value;
            applied.push("core.policies.tools.candidate_pool_size".to_string());
        }
        if let Some(value) =
            bool_setting(updates, "core.policies.tools.llm_candidate_filter_enabled")
        {
            self.tools.llm_candidate_filter_enabled = value;
            applied.push("core.policies.tools.llm_candidate_filter_enabled".to_string());
        }
        if let Some(value) = usize_setting(
            updates,
            "core.policies.tools.llm_candidate_filter_threshold",
        ) {
            self.tools.llm_candidate_filter_threshold = value;
            applied.push("core.policies.tools.llm_candidate_filter_threshold".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.tools.allow_plugin_tools") {
            self.tools.allow_plugin_tools = value;
            applied.push("core.policies.tools.allow_plugin_tools".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.tools.indexed_search_enabled") {
            self.tools.indexed_search_enabled = value;
            applied.push("core.policies.tools.indexed_search_enabled".to_string());
        }
        if let Some(value) =
            f32_setting(updates, "core.policies.tools.indexed_search_min_similarity")
        {
            self.tools.indexed_search_min_similarity = value.clamp(0.0, 1.0);
            applied.push("core.policies.tools.indexed_search_min_similarity".to_string());
        }
        if let Some(value) = bool_setting(
            updates,
            "core.policies.tools.indexed_search_refresh_missing_embeddings",
        ) {
            self.tools.indexed_search_refresh_missing_embeddings = value;
            applied
                .push("core.policies.tools.indexed_search_refresh_missing_embeddings".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.transcript_correction.enabled") {
            self.transcript_correction.enabled = value;
            applied.push("core.policies.transcript_correction.enabled".to_string());
        }
        if let Some(value) =
            bool_setting(updates, "core.policies.transcript_correction.apply_to_stt")
        {
            self.transcript_correction.apply_to_stt = value;
            applied.push("core.policies.transcript_correction.apply_to_stt".to_string());
        }
        if let Some(value) = string_setting(updates, "core.policies.transcript_correction.provider")
        {
            self.transcript_correction.provider = value;
            applied.push("core.policies.transcript_correction.provider".to_string());
        }
        if let Some(value) = u64_setting(updates, "core.policies.transcript_correction.timeout_sec")
        {
            self.transcript_correction.timeout_sec = value.max(1);
            applied.push("core.policies.transcript_correction.timeout_sec".to_string());
        }
        if let Some(value) = usize_setting(
            updates,
            "core.policies.transcript_correction.max_input_chars",
        ) {
            self.transcript_correction.max_input_chars = value;
            applied.push("core.policies.transcript_correction.max_input_chars".to_string());
        }
        if let Some(value) = f32_setting(
            updates,
            "core.policies.transcript_correction.min_confidence_to_replace",
        ) {
            self.transcript_correction.min_confidence_to_replace = value.clamp(0.0, 1.0);
            applied
                .push("core.policies.transcript_correction.min_confidence_to_replace".to_string());
        }
        if let Some(value) = bool_setting(
            updates,
            "core.policies.transcript_correction.require_confirmation_on_medium_risk",
        ) {
            self.transcript_correction
                .require_confirmation_on_medium_risk = value;
            applied.push(
                "core.policies.transcript_correction.require_confirmation_on_medium_risk"
                    .to_string(),
            );
        }
        if let Some(value) =
            bool_setting(updates, "core.policies.transcript_correction.trace_enabled")
        {
            self.transcript_correction.trace_enabled = value;
            applied.push("core.policies.transcript_correction.trace_enabled".to_string());
        }

        applied
    }
}

/// Spawn the pipeline processing loop.
///
/// The task runs until the cancel token is triggered. It subscribes to the
/// EventBus, filters for `ChannelInput` events, processes them through the
/// AI pipeline, and sends responses back via the channel registry.
/// Starts the bounded request supervisor. The owner must await the returned
/// task before shutting down modules/storage; dropping it does not join workers.
pub fn spawn_pipeline(
    event_bus: &EventBus,
    modules: Arc<ModuleRegistry>,
    channels: Arc<ChannelRegistry>,
    sensors: Arc<SensorRegistry>,
    cancel_token: CancellationToken,
    debug_output: bool,
    policies: SharedPipelinePolicies,
    mut voice_rx: tokio::sync::mpsc::Receiver<RecognizedVoiceInput>,
) -> tokio::task::JoinHandle<()> {
    let mut rx = event_bus.subscribe();
    let bus = event_bus.clone();
    let voice_turns = Arc::new(VoiceTurnRegistry::default());
    let shutdown = cancel_token.child_token();
    tokio::spawn(async move {
        let mut tasks = supervision::PipelineTasks::default();
        let mut pending = std::collections::VecDeque::new();
        let mut voice_open = true;
        loop {
            if shutdown.is_cancelled() {
                break;
            }
            // The reliable voice receiver keeps its own bounded queue. Legacy
            // broadcast events have a separate bounded, best-effort queue.
            let next = tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tasks.join_next(), if !tasks.is_empty() => continue,
                input = voice_rx.recv(), if voice_open && tasks.has_capacity() => {
                    match input {
                        Some(input) if !input.cancel.is_cancelled() => Some(voice::PipelineInput::Recognized(input)),
                        Some(_) => continue,
                        None => { voice_open = false; continue; }
                    }
                }
                _ = std::future::ready(()), if tasks.has_capacity() && !pending.is_empty() => {
                    pending.pop_front().map(voice::PipelineInput::Legacy)
                }
                event = rx.recv() => {
                    match event {
                        Ok(ServerEvent::ChannelInput(event @ ChannelEvent::InterruptActiveTurn { .. })) => {
                            let snapshot = policies.read().await.clone();
                            // Cancellation is delivered outside the request queue. Adapter
                            // stop notifications are bounded so they cannot wedge intake.
                            if tokio::time::timeout(std::time::Duration::from_secs(2), process_event(
                                event, &modules, &channels, &sensors, &bus, &voice_turns,
                                shutdown.child_token(), debug_output, &snapshot.conversation,
                                &snapshot.tools, &snapshot.cache, &snapshot.cache_policy,
                                &snapshot.memory, &snapshot.transcript_correction,
                            )).await.is_err() {
                                tracing::warn!("channel interrupt notification timed out");
                            }
                            continue;
                        }
                        Ok(ServerEvent::ChannelInput(event)) => {
                            if pending.len() < supervision::LEGACY_QUEUE_CAPACITY {
                                pending.push_back(event);
                            } else {
                                tracing::warn!(channel_id = channel_event_id(&event), "legacy pipeline input rejected: queue full");
                            }
                            continue;
                        }
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                            tracing::warn!(missed, "legacy pipeline event bus lagged");
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            };
            let Some(input) = next else {
                continue;
            };
            let token = match &input {
                voice::PipelineInput::Recognized(input) => input.cancel.clone(),
                voice::PipelineInput::Legacy(_) => shutdown.child_token(),
            };
            if shutdown.is_cancelled() {
                token.cancel();
                break;
            }
            if token.is_cancelled() {
                continue;
            }
            let modules = modules.clone();
            let channels = channels.clone();
            let sensors = sensors.clone();
            let bus = bus.clone();
            let voice_turns = voice_turns.clone();
            let policies = policies.clone();
            let worker_token = token.clone();
            tasks.spawn(token, async move {
                // Policy reads may yield; cancellation must be checked again by
                // the worker before it touches STT, context, or an LLM.
                let policy = policies.read().await.clone();
                let result = match input {
                    voice::PipelineInput::Legacy(event) => {
                        process_event(
                            event,
                            &modules,
                            &channels,
                            &sensors,
                            &bus,
                            &voice_turns,
                            worker_token,
                            debug_output,
                            &policy.conversation,
                            &policy.tools,
                            &policy.cache,
                            &policy.cache_policy,
                            &policy.memory,
                            &policy.transcript_correction,
                        )
                        .await
                    }
                    voice::PipelineInput::Recognized(input) => {
                        voice::process_recognized_voice(
                            input,
                            &modules,
                            &channels,
                            &bus,
                            &voice_turns,
                            debug_output,
                            &policy,
                        )
                        .await
                    }
                };
                if let Err(error) = result {
                    tracing::error!(error = %error, "pipeline request failed");
                    tracing::debug!(error = ?error, "pipeline request failure details");
                }
            });
        }
        shutdown.cancel();
        voice_rx.close();
        while let Some(input) = voice_rx.recv().await {
            input.cancel.cancel();
        }
        pending.clear();
        // No abort_all: already-dispatched tools must complete and record their
        // result before their cancelled response is discarded.
        tasks.shutdown().await;
        tracing::info!("pipeline requests drained");
    })
}

/// Process a single ChannelEvent through the AI pipeline.
async fn process_event(
    event: ChannelEvent,
    modules: &ModuleRegistry,
    channels: &ChannelRegistry,
    sensors: &SensorRegistry,
    event_bus: &EventBus,
    voice_turns: &VoiceTurnRegistry,
    request_cancel: CancellationToken,
    debug_output: bool,
    conversation_config: &ConversationSection,
    tools_config: &ToolsSection,
    cache_config: &CacheSection,
    cache_policy: &CachePolicyResourceSection,
    memory_config: &MemorySection,
    transcript_correction_config: &crate::config::TranscriptCorrectionPolicySection,
) -> anyhow::Result<()> {
    if request_cancel.is_cancelled() {
        return Ok(());
    }
    match event {
        ChannelEvent::TextMessage {
            channel_id,
            user_id,
            text,
        } => {
            debug_component_text(
                debug_output,
                "pipeline",
                "text_input",
                &text,
                Some((&channel_id, &user_id)),
            );
            let (channel_id, channel) = resolve_event_channel(channels, &channel_id).await?;
            let target = ChannelTarget {
                voice: None,
                user_id: user_id.clone(),
            };

            let turn = VoiceTurn::request(request_cancel);
            let result = match process_text(
                &text,
                &user_id,
                modules,
                channel.as_ref(),
                &target,
                &channel_id,
                event_bus,
                debug_output,
                false,
                false,
                conversation_config,
                tools_config,
                cache_config,
                cache_policy,
                memory_config,
                Some(&turn),
                true,
                None,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    notify_pipeline_error(
                        modules,
                        channel.as_ref(),
                        &target,
                        &channel_id,
                        &user_id,
                        PipelineErrorStage::Llm,
                        Some(&text),
                        Some(&error),
                        debug_output,
                        false,
                        Some(&turn),
                    )
                    .await;
                    return Ok(());
                }
            };

            if result.interrupted || turn_is_interrupted(Some(&turn)) {
                return Ok(());
            }
            // Only call send_message when the text was NOT already streamed.
            if !result.streamed {
                debug_component_text(
                    debug_output,
                    "channel",
                    "send_message",
                    &result.text,
                    Some((&channel_id, &user_id)),
                );
                if let Err(error) = cancellable_stage(
                    Some(&turn),
                    "text_delivery",
                    channel.send_message(&target, &result.text),
                )
                .await
                {
                    log_channel_delivery_failure(channel.as_ref(), error).await;
                }
            }
            if let Some(plan) = &result.expression_plan {
                execute_expression_and_trace(
                    channel.as_ref(),
                    &target,
                    plan,
                    None,
                    debug_output,
                    &channel_id,
                    &user_id,
                    Some(&turn),
                )
                .await;
            }

            if !turn_is_interrupted(Some(&turn)) {
                if let Some(pending) = result.pending_persistence {
                    persist_text_interaction(modules, pending, memory_config, event_bus, None)
                        .await;
                }
            }
            Ok(())
        }
        ChannelEvent::InterruptActiveTurn {
            channel_id,
            user_id,
            reason,
        } => {
            let (channel_id, channel) = match resolve_event_channel(channels, &channel_id).await {
                Ok(resolved) => resolved,
                Err(error) => {
                    tracing::warn!(
                        requested_channel_id = %channel_id,
                        user_id,
                        reason,
                        error = %error,
                        "interrupt requested for an unknown channel"
                    );
                    return Ok(());
                }
            };
            let interrupted = voice_turns.interrupt(&channel_id, &user_id, &reason).await;
            if interrupted {
                tracing::info!(channel_id, user_id, reason, "active voice turn interrupted");
                if reason == "wakeword_during_thinking" {
                    debug_component_json(
                        debug_output,
                        "thinking_cue",
                        "interrupt",
                        &serde_json::json!({
                            "reason": reason.as_str(),
                            "channel_id": channel_id.as_str(),
                            "user_id": user_id.as_str(),
                        }),
                        Some((&channel_id, &user_id)),
                    );
                }
                let config = channel.interrupt_config();
                if config.enabled {
                    let target = ChannelTarget {
                        voice: None,
                        user_id: user_id.clone(),
                    };
                    if let Err(error) = channel.stop_current_playback(&target, &reason).await {
                        tracing::warn!(
                            channel_type = channel.channel_type(),
                            channel_id = channel.channel_id(),
                            error = %error,
                            "failed to stop channel playback after interrupt"
                        );
                        tracing::debug!(error = ?error, "stop playback failure details");
                    }
                    let expression_cancel =
                        cancel_expression_execution(channel.as_ref(), &target, &reason).await;
                    debug_component_text(
                        debug_output,
                        "expression",
                        "cancel",
                        &serde_json::to_string(&expression_cancel)
                            .unwrap_or_else(|_| expression_cancel.status),
                        Some((&channel_id, &user_id)),
                    );
                }
            } else {
                tracing::debug!(
                    channel_id,
                    user_id,
                    reason,
                    "interrupt requested but no active voice turn was registered"
                );
            }
            Ok(())
        }
        ChannelEvent::AudioInput {
            channel_id,
            user_id,
            audio,
        } => {
            let (channel_id, channel) = resolve_event_channel(channels, &channel_id).await?;
            let target = ChannelTarget {
                voice: None,
                user_id: user_id.clone(),
            };
            let turn = voice_turns
                .begin(
                    channel_id.clone(),
                    user_id.clone(),
                    channel.interrupt_config(),
                    request_cancel,
                )
                .await;

            // STT → Router → LLM → TTS
            debug_component_audio(
                debug_output,
                "stt",
                "call",
                audio.data.len(),
                Some((&channel_id, &user_id)),
            );
            let (stt_audio, stt_encoding) = prepare_audio_for_stt(&audio);
            let transcription = match cancellable_stage(
                Some(&turn),
                "stt",
                modules.stt.transcribe_result(&stt_audio, stt_encoding),
            )
            .await
            {
                Ok(Some(transcription)) => transcription,
                Ok(None) => {
                    voice_turns.finish(&turn).await;
                    return Ok(());
                }
                Err(error) => {
                    notify_pipeline_error(
                        modules,
                        channel.as_ref(),
                        &target,
                        &channel_id,
                        &user_id,
                        PipelineErrorStage::Stt,
                        None,
                        Some(&error),
                        debug_output,
                        true,
                        Some(&turn),
                    )
                    .await;
                    voice_turns.finish(&turn).await;
                    return Ok(());
                }
            };
            let stt_capabilities = modules.stt.stt_capabilities().await;
            let transcript = transcription.text.clone();
            debug_component_text(
                debug_output,
                "stt",
                "result",
                &transcript,
                Some((&channel_id, &user_id)),
            );

            if transcript.trim().is_empty() {
                debug_component(
                    debug_output,
                    "stt",
                    "empty_result",
                    Some((&channel_id, &user_id)),
                );
                voice_turns.finish(&turn).await;
                return Ok(());
            }

            let correction = cancellable_stage(Some(&turn), "transcript_correction", async {
                let correction_context = build_transcript_correction_context(
                    modules,
                    sensors,
                    &channel_id,
                    &user_id,
                    conversation_config,
                )
                .await;
                let correction = correct_stt_transcript(
                    &transcription,
                    &stt_capabilities,
                    &correction_context,
                    transcript_correction_config,
                    transcript_correction_llm(modules, transcript_correction_config),
                )
                .await;
                Ok(correction)
            })
            .await;
            let correction = match correction {
                Ok(Some(correction)) => correction,
                Ok(None) => {
                    voice_turns.finish(&turn).await;
                    return Ok(());
                }
                Err(error) => {
                    voice_turns.finish(&turn).await;
                    return Err(error);
                }
            };
            if correction.requires_user_confirmation(transcript_correction_config) {
                let clarification = correction.clarification_prompt();
                handle_transcript_correction_confirmation(
                    modules,
                    channel.as_ref(),
                    &target,
                    &channel_id,
                    &user_id,
                    &clarification,
                    &correction,
                    debug_output,
                    Some(&turn),
                )
                .await;
                voice_turns.finish(&turn).await;
                return Ok(());
            }
            let corrected_text = correction
                .text_for_processing(transcript_correction_config)
                .to_string();
            let correction_trace = transcript_correction_config
                .trace_enabled
                .then_some(correction.clone());
            debug_component_json(
                debug_output,
                "stt",
                "correction_detail",
                &transcript_correction_debug_payload(&correction, &corrected_text),
                Some((&channel_id, &user_id)),
            );
            debug_component_text(
                debug_output,
                "stt",
                "corrected",
                &corrected_text,
                Some((&channel_id, &user_id)),
            );

            let result = voice::process_voice_response(
                &corrected_text,
                &user_id,
                modules,
                channel.as_ref(),
                &target,
                &channel_id,
                event_bus,
                debug_output,
                conversation_config,
                tools_config,
                cache_config,
                cache_policy,
                memory_config,
                &turn,
                correction_trace,
            )
            .await;
            voice_turns.finish(&turn).await;
            result
        }
        ChannelEvent::SensorData {
            channel_id,
            sensor_type,
            payload,
        } => {
            let (channel_id, _channel) = resolve_event_channel(channels, &channel_id).await?;
            // Ingest sensor data into the registry
            use crate::providers::sensor::SensorReading;
            let sensor_id = format!("{}:{}", channel_id, sensor_type);
            let reading = SensorReading {
                sensor_id: sensor_id.clone(),
                value: payload.clone(),
                timestamp: chrono::Utc::now(),
                quality: 1.0,
            };
            let source = format!("component:{channel_id}");
            sensors.ingest_from_source(reading, &source).await?;
            event_bus.publish(ServerEvent::SensorDataIngested {
                sensor_id,
                source,
                value: payload,
            });
            Ok(())
        }
        ChannelEvent::StatusChange { .. } => Ok(()),
    }
}

fn prepare_audio_for_stt(audio: &AudioPayload) -> (Vec<u8>, AudioEncoding) {
    match audio.format {
        AudioFormat::Opus => (audio.data.clone(), AudioEncoding::Opus),
        AudioFormat::Wav => (audio.data.clone(), AudioEncoding::Wav),
        AudioFormat::Pcm16 if audio.sample_rate == 16_000 && audio.channels == 1 => {
            (audio.data.clone(), AudioEncoding::Pcm16kMono)
        }
        AudioFormat::Pcm16 => (
            pcm16le_to_wav(&audio.data, audio.sample_rate, audio.channels),
            AudioEncoding::Wav,
        ),
    }
}

fn pcm16le_to_wav(pcm: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let channels = channels.max(1);
    let sample_rate = sample_rate.max(1);
    let data_len = pcm.len() as u32;
    let byte_rate = sample_rate * channels as u32 * 2;
    let block_align = channels * 2;

    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

fn channel_event_id(event: &ChannelEvent) -> Option<&str> {
    match event {
        ChannelEvent::TextMessage { channel_id, .. }
        | ChannelEvent::AudioInput { channel_id, .. }
        | ChannelEvent::InterruptActiveTurn { channel_id, .. }
        | ChannelEvent::SensorData { channel_id, .. }
        | ChannelEvent::StatusChange { channel_id, .. } => Some(channel_id),
    }
}

async fn resolve_event_channel(
    channels: &ChannelRegistry,
    channel_id: &str,
) -> anyhow::Result<(String, Arc<dyn ChannelAdapter>)> {
    channels
        .get_resolved(channel_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("channel '{}' not found for response delivery", channel_id))
}

fn stable_user_uuid(user_id: &str) -> Uuid {
    let user_id = user_id.trim();
    let stable_key = if user_id.is_empty() {
        "unknown-pipeline-user"
    } else {
        user_id
    };
    Uuid::parse_str(stable_key).unwrap_or_else(|_| {
        let digest = Sha256::digest(stable_key.as_bytes());
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x50;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid::from_bytes(bytes)
    })
}

/// Core text processing: Router → Cache/Skill/LLM → response text.
///
/// Returns a [`TextResult`] containing the full response and a flag indicating
/// whether the text was already delivered to the channel via streaming.
///
/// Streaming is used when ALL of these are true:
/// 1. The route decision is an LLM call (not cache/skill)
/// 2. The channel reports `capabilities().streaming == true`
///
/// For non-streaming paths (cache hit, skill, or non-streaming channel) the
/// caller is responsible for calling `send_message` with the returned text.
async fn process_text(
    text: &str,
    user_id: &str,
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    event_bus: &EventBus,
    debug_output: bool,
    audio_response: bool,
    input_from_stt: bool,
    conversation_config: &ConversationSection,
    tools_config: &ToolsSection,
    cache_config: &CacheSection,
    cache_policy: &CachePolicyResourceSection,
    memory_config: &MemorySection,
    turn: Option<&VoiceTurn>,
    defer_persistence: bool,
    transcript_correction: Option<TranscriptCorrectionResult>,
) -> anyhow::Result<TextResult> {
    if turn_is_interrupted(turn) {
        return Ok(TextResult::interrupted());
    }
    let uid = stable_user_uuid(user_id);
    let (mut base_system_messages, runtime_context) = match cancellable_stage(
        turn,
        "context",
        build_base_system_messages(uid, modules, memory_config, text),
    )
    .await?
    {
        Some(context) => context,
        None => return Ok(TextResult::interrupted()),
    };
    if input_from_stt {
        base_system_messages.push(system_message(stt_transcript_context_prompt()));
    }
    if audio_response {
        base_system_messages.push(system_message(tts_optimized_output_prompt(text)));
    }
    let memory_context_used = runtime_context_has_user_memory(&runtime_context);

    // 1. Get active session and load history
    let session_id = match cancellable_stage(
        turn,
        "conversation_session",
        modules
            .conversation
            .get_or_create_active_session(channel_id, user_id),
    )
    .await?
    {
        Some(session_id) => session_id,
        None => return Ok(TextResult::interrupted()),
    };
    let history = match cancellable_stage(
        turn,
        "conversation_history",
        modules.conversation.get_history(
            session_id,
            conversation_config.max_context_messages.max(1) as u64,
        ),
    )
    .await?
    {
        Some(history) => history,
        None => return Ok(TextResult::interrupted()),
    };
    let prior_message_count = history.len();
    let mut trace = AgentTrace::new(session_id);
    if let Some(turn) = turn {
        trace.turn_id = turn.id;
    }
    trace
        .record(
            modules,
            "turn_started",
            "pipeline",
            Some("User turn started".to_string()),
            serde_json::json!({
                "channel_id": channel_id,
                "user_id": user_id,
                "input_preview": preview_for_prompt(text, 240),
                "history_messages": history.len(),
                "voice_scope": turn.and_then(|turn| turn.voice_scope.as_ref()),
                "memory_context_used": memory_context_used,
            }),
        )
        .await;
    if let Some(correction) = transcript_correction.as_ref()
        && correction.status != transcript_correction::TranscriptCorrectionStatus::Disabled
    {
        trace
            .record(
                modules,
                "stt_correction",
                "stt",
                Some("STT transcript correction completed".to_string()),
                transcript_correction_trace_payload(correction, text),
            )
            .await;
    }
    match cancellable_stage(turn, "observe_user_affect_for_turn", async {
        Ok(observe_user_affect_for_turn(
            modules,
            &mut trace,
            channel_id,
            user_id,
            text,
            debug_output,
        )
        .await)
    })
    .await?
    {
        Some(value) => value,
        None => return Ok(TextResult::interrupted()),
    };

    // 2. Check exact cache first (never streamed). Only entries with explicit
    // safe-direct metadata can bypass the LLM.
    if cache_config.enabled && cache_config.exact_match_enabled && !memory_context_used {
        if let Some(cached) =
            cancellable_stage(turn, "cache_lookup", modules.cache.get(Some(uid), text))
                .await?
                .flatten()
        {
            if cached_response_is_safe_direct(&cached) {
                let cache_metadata = cached.metadata.clone();
                let response_text = cached.response_text;
                tracing::debug!(query = text, "pipeline cache hit");
                debug_component_text(
                    debug_output,
                    "cache",
                    "hit",
                    &response_text,
                    Some((channel_id, user_id)),
                );
                trace
                    .record(
                        modules,
                        "cache_hit",
                        "cache",
                        Some("Direct cache hit".to_string()),
                        serde_json::json!({
                            "kind": "exact",
                            "metadata": cache_metadata,
                        }),
                    )
                    .await;
                let pending = PendingTextPersistence {
                    session_id,
                    user_id: uid,
                    user_id_text: user_id.to_string(),
                    query_text: text.to_string(),
                    response_text: response_text.clone(),
                    channel_id: channel_id.to_string(),
                    cache_write: None,
                };
                if !defer_persistence {
                    persist_text_interaction(
                        modules,
                        pending.clone(),
                        memory_config,
                        event_bus,
                        Some(&mut trace),
                    )
                    .await;
                }
                match cancellable_stage(turn, "observe_assistant_affect_for_turn", async {
                    Ok(observe_assistant_affect_for_turn(
                        modules,
                        &mut trace,
                        channel_id,
                        user_id,
                        &response_text,
                        "cache",
                        false,
                        false,
                        debug_output,
                    )
                    .await)
                })
                .await?
                {
                    Some(value) => value,
                    None => return Ok(TextResult::interrupted()),
                };
                trace
                    .record(
                        modules,
                        "turn_completed",
                        "pipeline",
                        Some("Turn completed from cache".to_string()),
                        serde_json::json!({
                            "route": "cache",
                            "streamed": false,
                            "audio_streamed": false,
                            "used_tools": false,
                            "response_chars": response_text.chars().count(),
                        }),
                    )
                    .await;
                let planned_expression =
                    match cancellable_stage(turn, "plan_expression_for_response", async {
                        Ok(plan_expression_for_response(
                            modules,
                            &mut trace,
                            channel,
                            channel_id,
                            user_id,
                            &response_text,
                            "cache",
                            false,
                            false,
                            true,
                            text,
                            prior_message_count,
                            memory_context_used,
                            memory_config,
                            debug_output,
                        )
                        .await)
                    })
                    .await?
                    {
                        Some(value) => value,
                        None => return Ok(TextResult::interrupted()),
                    };
                let tts_trace = TtsTraceContext::from_agent_trace(modules, &trace);
                return Ok(TextResult {
                    text: response_text.clone(),
                    streamed: false,
                    audio_streamed: false,
                    interrupted: false,
                    pending_persistence: defer_persistence.then_some(pending),
                    expression_plan: Some(planned_expression.plan),
                    expression_input: Some(planned_expression.input),
                    tts_trace: Some(tts_trace),
                });
            }
            tracing::debug!(query = text, "cache candidate ignored by safety policy");
            debug_component(
                debug_output,
                "cache",
                "unsafe_hit_ignored",
                Some((channel_id, user_id)),
            );
            trace
                .record(
                    modules,
                    "cache_ignored",
                    "cache",
                    Some("Exact cache candidate ignored by safety policy".to_string()),
                    serde_json::json!({
                        "kind": "exact",
                        "metadata": cached.metadata,
                    }),
                )
                .await;
        }
    }
    debug_component(debug_output, "cache", "miss", Some((channel_id, user_id)));
    trace
        .record(
            modules,
            "cache_miss",
            "cache",
            Some("No direct cache response used".to_string()),
            serde_json::json!({
                "exact_enabled": cache_config.exact_match_enabled,
                "semantic_enabled": cache_config.semantic_match_enabled,
                "memory_context_used": memory_context_used,
            }),
        )
        .await;

    if cache_config.enabled && cache_config.semantic_match_enabled && !memory_context_used {
        let semantic_candidates =
            match cancellable_stage(turn, "semantic_cache_candidates", async {
                Ok(semantic_cache_candidates(modules, uid, text, debug_output).await)
            })
            .await?
            {
                Some(value) => value,
                None => return Ok(TextResult::interrupted()),
            };
        if let Some(candidate) = semantic_candidates
            .iter()
            .find(|candidate| semantic_candidate_is_safe_direct(candidate))
        {
            let cache_metadata = candidate.metadata.clone();
            let response_text = candidate.response_text.clone();
            let similarity = candidate.similarity;
            tracing::debug!(query = text, similarity, "pipeline cache hit (semantic)");
            debug_component_text(
                debug_output,
                "cache",
                "semantic_hit",
                &response_text,
                Some((channel_id, user_id)),
            );
            trace
                .record(
                    modules,
                    "cache_hit",
                    "cache",
                    Some("Direct semantic cache hit".to_string()),
                    serde_json::json!({
                        "kind": "semantic",
                        "similarity": similarity,
                        "metadata": cache_metadata,
                    }),
                )
                .await;
            let pending = PendingTextPersistence {
                session_id,
                user_id: uid,
                user_id_text: user_id.to_string(),
                query_text: text.to_string(),
                response_text: response_text.clone(),
                channel_id: channel_id.to_string(),
                cache_write: None,
            };
            if !defer_persistence {
                persist_text_interaction(
                    modules,
                    pending.clone(),
                    memory_config,
                    event_bus,
                    Some(&mut trace),
                )
                .await;
            }
            match cancellable_stage(turn, "observe_assistant_affect_for_turn", async {
                Ok(observe_assistant_affect_for_turn(
                    modules,
                    &mut trace,
                    channel_id,
                    user_id,
                    &response_text,
                    "cache",
                    false,
                    false,
                    debug_output,
                )
                .await)
            })
            .await?
            {
                Some(value) => value,
                None => return Ok(TextResult::interrupted()),
            };
            trace
                .record(
                    modules,
                    "turn_completed",
                    "pipeline",
                    Some("Turn completed from semantic cache".to_string()),
                    serde_json::json!({
                        "route": "cache",
                        "cache_kind": "semantic",
                        "similarity": similarity,
                        "streamed": false,
                        "audio_streamed": false,
                        "used_tools": false,
                        "response_chars": response_text.chars().count(),
                    }),
                )
                .await;
            let planned_expression =
                match cancellable_stage(turn, "plan_expression_for_response", async {
                    Ok(plan_expression_for_response(
                        modules,
                        &mut trace,
                        channel,
                        channel_id,
                        user_id,
                        &response_text,
                        "cache",
                        false,
                        false,
                        true,
                        text,
                        prior_message_count,
                        memory_context_used,
                        memory_config,
                        debug_output,
                    )
                    .await)
                })
                .await?
                {
                    Some(value) => value,
                    None => return Ok(TextResult::interrupted()),
                };
            let tts_trace = TtsTraceContext::from_agent_trace(modules, &trace);
            return Ok(TextResult {
                text: response_text.clone(),
                streamed: false,
                audio_streamed: false,
                interrupted: false,
                pending_persistence: defer_persistence.then_some(pending),
                expression_plan: Some(planned_expression.plan),
                expression_input: Some(planned_expression.input),
                tts_trace: Some(tts_trace),
            });
        }

        if let Some((prompt, candidate_count)) =
            semantic_cache_reference_prompt(&semantic_candidates, debug_output)
        {
            trace
                .record(
                    modules,
                    "semantic_cache_candidates",
                    "cache",
                    Some("Semantic cache references attached".to_string()),
                    serde_json::json!({
                        "candidates": candidate_count,
                    }),
                )
                .await;
            base_system_messages.push(system_message(prompt));
        }
    }

    // 3. Route the input
    debug_component_text(
        debug_output,
        "router",
        "call",
        text,
        Some((channel_id, user_id)),
    );
    let route_input = RouterInput {
        text: text.to_string(),
        user_id: uid,
        conversation_context: history.clone(),
        assistant_name: runtime_context.assistant_name.clone(),
    };
    let decision =
        match cancellable_stage(turn, "router", modules.router.route(&route_input)).await? {
            Some(decision) => decision,
            None => {
                if let Some(turn) = turn {
                    persist_interrupted_input_context(
                        modules,
                        session_id,
                        uid,
                        user_id,
                        text,
                        channel_id,
                        turn,
                        debug_output,
                    )
                    .await;
                }
                trace
                    .record(
                        modules,
                        "turn_interrupted",
                        "pipeline",
                        Some("Turn interrupted during router stage".to_string()),
                        serde_json::json!({
                            "stage": "router",
                        }),
                    )
                    .await;
                return Ok(TextResult {
                    text: String::new(),
                    streamed: false,
                    audio_streamed: false,
                    interrupted: true,
                    pending_persistence: None,
                    expression_plan: None,
                    expression_input: None,
                    tts_trace: Some(TtsTraceContext::from_agent_trace(modules, &trace)),
                });
            }
        };
    debug_component_route(
        debug_output,
        "router",
        "result",
        route_decision_name(&decision),
        Some((channel_id, user_id)),
    );
    trace
        .record(
            modules,
            "route_decision",
            "router",
            Some(route_decision_name(&decision).to_string()),
            serde_json::json!({
                "route": route_decision_name(&decision),
            }),
        )
        .await;

    let (outcome, skill_cacheable, route_for_cache) = match decision {
        // Cache hit from router — never streamed
        RouteDecision::CacheHit { response } => {
            debug_component_text(
                debug_output,
                "router_cache",
                "result",
                &response,
                Some((channel_id, user_id)),
            );
            (
                LlmExecutionOutcome {
                    text: response,
                    streamed: false,
                    audio_streamed: false,
                    used_tools: false,
                },
                None,
                "router_cache",
            )
        }

        // Skill execution — resolve skill, then send through LLM with tools.
        // Skills extend what the LLM knows how to do well; the LLM decides
        // which tools to call (if any) based on the skill's system prompt.
        RouteDecision::Skill { skill_name } => {
            let skill_match = crate::providers::adapter::SkillMatch {
                name: skill_name,
                confidence: 1.0,
            };
            let resolution = match cancellable_stage(
                turn,
                "skill_context",
                modules.skills.resolve(&skill_match),
            )
            .await?
            {
                Some(resolution) => resolution,
                None => return Ok(TextResult::interrupted()),
            };
            trace
                .record(
                    modules,
                    "skill_resolved",
                    "skills",
                    Some(resolution.skill_name.clone()),
                    serde_json::json!({
                        "active_skills": [resolution.skill_name.clone()],
                        "cacheable": resolution.cacheable,
                        "preferred_tools": resolution.preferred_tools.clone(),
                    }),
                )
                .await;

            // Build messages with the skill's domain-specific system prompt
            let mut messages = base_system_messages.clone();
            if let Some(sp) = &resolution.system_prompt {
                messages.push(ChatMessage {
                    role: "system".into(),
                    content: sp.clone(),
                    name: None,
                    ..ChatMessage::default()
                });
            }
            messages.push(ChatMessage {
                role: "system".into(),
                content: "The active skill provides domain context only. Preferred tools are ranking hints, not restrictions; use whichever available tool best matches the user's request.".to_string(),
                name: None,
                ..ChatMessage::default()
            });
            append_history_with_interrupted_context(&mut messages, &history);
            messages.push(ChatMessage {
                role: "user".into(),
                content: text.to_string(),
                name: Some(user_id.to_string()),
                ..ChatMessage::default()
            });

            let tool_selection = match cancellable_stage(
                turn,
                "select_tools_for_skill",
                select_tools_for_skill(
                    modules,
                    &resolution.preferred_tools,
                    text,
                    tools_config,
                    channel_id,
                    &runtime_context.assistant_name,
                ),
            )
            .await?
            {
                Some(value) => value,
                None => return Ok(TextResult::interrupted()),
            };
            let skill_cacheable = resolution.cacheable;
            let sentence_speech_context =
                match cancellable_stage(turn, "build_sentence_speech_context", async {
                    Ok(build_sentence_speech_context(
                        modules,
                        channel,
                        channel_id,
                        user_id,
                        "skill",
                        false,
                        false,
                        false,
                        text,
                        prior_message_count,
                        memory_context_used,
                        runtime_context.memory_context.clone(),
                        audio_response
                            && allow_streaming_for_tools(
                                &tool_selection.selected_tools,
                                conversation_config.streaming_enabled,
                                channel,
                            ),
                    )
                    .await)
                })
                .await?
                {
                    Some(value) => value,
                    None => return Ok(TextResult::interrupted()),
                };
            let outcome = execute_llm_response(
                modules,
                modules.llm.as_ref(),
                messages,
                tool_selection,
                uid,
                session_id,
                channel,
                target,
                channel_id,
                user_id,
                event_bus,
                "skill",
                audio_response,
                conversation_config.streaming_enabled,
                tools_config,
                debug_output,
                &mut trace,
                turn,
                sentence_speech_context,
            )
            .await?;
            (outcome, Some(skill_cacheable), "skill")
        }

        // LLM call — stream when the channel supports it
        RouteDecision::MinorLlm {
            system_prompt,
            escalation_prompt,
        } => {
            let messages = build_llm_messages(
                &base_system_messages,
                &history,
                system_prompt,
                text,
                user_id,
            );
            let tool_selection = match cancellable_stage(
                turn,
                "tool_selection",
                select_all_tools(
                    modules,
                    text,
                    tools_config,
                    channel_id,
                    &runtime_context.assistant_name,
                ),
            )
            .await?
            {
                Some(selection) => selection,
                None => return Ok(TextResult::interrupted()),
            };
            let allow_minor_streaming =
                conversation_config.streaming_enabled && escalation_prompt.is_none();
            let sentence_speech_context =
                match cancellable_stage(turn, "build_sentence_speech_context", async {
                    Ok(build_sentence_speech_context(
                        modules,
                        channel,
                        channel_id,
                        user_id,
                        "minor_llm",
                        false,
                        false,
                        false,
                        text,
                        prior_message_count,
                        memory_context_used,
                        runtime_context.memory_context.clone(),
                        audio_response
                            && allow_streaming_for_tools(
                                &tool_selection.selected_tools,
                                allow_minor_streaming,
                                channel,
                            ),
                    )
                    .await)
                })
                .await?
                {
                    Some(value) => value,
                    None => return Ok(TextResult::interrupted()),
                };
            let minor_outcome = execute_llm_response(
                modules,
                modules.minor_llm.as_ref(),
                messages,
                tool_selection,
                uid,
                session_id,
                channel,
                target,
                channel_id,
                user_id,
                event_bus,
                "minor_llm",
                audio_response,
                allow_minor_streaming,
                tools_config,
                debug_output,
                &mut trace,
                turn,
                sentence_speech_context,
            )
            .await?;

            if let Some(reason) = escalation_reason(&minor_outcome.text)
                && let Some(major_prompt) = escalation_prompt
            {
                tracing::info!(
                    channel_id,
                    user_id,
                    reason = %reason,
                    "minor LLM escalated turn to major LLM"
                );
                debug_component_text(
                    debug_output,
                    "llm",
                    "escalate",
                    reason,
                    Some((channel_id, user_id)),
                );
                let mut major_messages = build_llm_messages(
                    &base_system_messages,
                    &history,
                    Some(major_prompt),
                    text,
                    user_id,
                );
                major_messages.push(ChatMessage::new(
                    "system",
                    format!(
                        "The minor model escalated this request instead of answering. Escalation reason: {reason}. Answer the original user request directly."
                    ),
                ));
                let major_tool_selection = match cancellable_stage(
                    turn,
                    "tool_selection",
                    select_all_tools(
                        modules,
                        text,
                        tools_config,
                        channel_id,
                        &runtime_context.assistant_name,
                    ),
                )
                .await?
                {
                    Some(selection) => selection,
                    None => return Ok(TextResult::interrupted()),
                };
                let sentence_speech_context =
                    match cancellable_stage(turn, "build_sentence_speech_context", async {
                        Ok(build_sentence_speech_context(
                            modules,
                            channel,
                            channel_id,
                            user_id,
                            "major_llm_escalated",
                            false,
                            false,
                            false,
                            text,
                            prior_message_count,
                            memory_context_used,
                            runtime_context.memory_context.clone(),
                            audio_response
                                && allow_streaming_for_tools(
                                    &major_tool_selection.selected_tools,
                                    conversation_config.streaming_enabled,
                                    channel,
                                ),
                        )
                        .await)
                    })
                    .await?
                    {
                        Some(value) => value,
                        None => return Ok(TextResult::interrupted()),
                    };
                let major_outcome = execute_llm_response(
                    modules,
                    modules.major_llm.as_ref(),
                    major_messages,
                    major_tool_selection,
                    uid,
                    session_id,
                    channel,
                    target,
                    channel_id,
                    user_id,
                    event_bus,
                    "major_llm_escalated",
                    audio_response,
                    conversation_config.streaming_enabled,
                    tools_config,
                    debug_output,
                    &mut trace,
                    turn,
                    sentence_speech_context,
                )
                .await?;
                (major_outcome, None, "major_llm_escalated")
            } else {
                (minor_outcome, None, "minor_llm")
            }
        }
        RouteDecision::MajorLlm { system_prompt } => {
            let messages = build_llm_messages(
                &base_system_messages,
                &history,
                system_prompt,
                text,
                user_id,
            );
            let tool_selection = match cancellable_stage(
                turn,
                "tool_selection",
                select_all_tools(
                    modules,
                    text,
                    tools_config,
                    channel_id,
                    &runtime_context.assistant_name,
                ),
            )
            .await?
            {
                Some(selection) => selection,
                None => return Ok(TextResult::interrupted()),
            };
            let sentence_speech_context =
                match cancellable_stage(turn, "build_sentence_speech_context", async {
                    Ok(build_sentence_speech_context(
                        modules,
                        channel,
                        channel_id,
                        user_id,
                        "major_llm",
                        false,
                        false,
                        false,
                        text,
                        prior_message_count,
                        memory_context_used,
                        runtime_context.memory_context.clone(),
                        audio_response
                            && allow_streaming_for_tools(
                                &tool_selection.selected_tools,
                                conversation_config.streaming_enabled,
                                channel,
                            ),
                    )
                    .await)
                })
                .await?
                {
                    Some(value) => value,
                    None => return Ok(TextResult::interrupted()),
                };
            let outcome = execute_llm_response(
                modules,
                modules.major_llm.as_ref(),
                messages,
                tool_selection,
                uid,
                session_id,
                channel,
                target,
                channel_id,
                user_id,
                event_bus,
                "major_llm",
                audio_response,
                conversation_config.streaming_enabled,
                tools_config,
                debug_output,
                &mut trace,
                turn,
                sentence_speech_context,
            )
            .await?;
            (outcome, None, "major_llm")
        }
    };

    if turn_is_interrupted(turn) {
        if let Some(turn) = turn {
            turn.mark_interrupted_stage("before_persistence");
            persist_interrupted_input_context(
                modules,
                session_id,
                uid,
                user_id,
                text,
                channel_id,
                turn,
                debug_output,
            )
            .await;
        }
        trace
            .record(
                modules,
                "turn_interrupted",
                "pipeline",
                Some("Turn interrupted before persistence".to_string()),
                serde_json::json!({
                    "route": route_for_cache,
                    "streamed": outcome.streamed,
                    "audio_streamed": outcome.audio_streamed,
                    "used_tools": outcome.used_tools,
                }),
            )
            .await;
        return Ok(TextResult {
            text: outcome.text,
            streamed: outcome.streamed,
            audio_streamed: outcome.audio_streamed,
            interrupted: true,
            pending_persistence: None,
            expression_plan: None,
            expression_input: None,
            tts_trace: Some(TtsTraceContext::from_agent_trace(modules, &trace)),
        });
    }

    let mut response_was_fallback = false;
    let used_tools = outcome.used_tools;
    let mut response = outcome.text;
    let mut streamed = outcome.streamed;
    let mut audio_streamed = outcome.audio_streamed;
    if response.trim().is_empty() {
        tracing::warn!(
            channel_id,
            user_id,
            "pipeline produced an empty assistant response; using fallback text"
        );
        response = EMPTY_RESPONSE_FALLBACK.to_string();
        streamed = false;
        audio_streamed = false;
        response_was_fallback = true;
    }

    let cache_context = CacheContext {
        memory_context_used,
        used_tools,
        skill_cacheable,
        response_was_fallback,
        route: route_for_cache,
    };
    let cache_write =
        cache_write_for_response(cache_config, cache_policy, text, &response, cache_context);
    match &cache_write {
        Some(write) => {
            trace
                .record(
                    modules,
                    "cache_write_scheduled",
                    "cache",
                    Some("Response eligible for cache write".to_string()),
                    serde_json::json!({
                        "scope": write.scope,
                        "ttl_seconds": write.ttl_seconds,
                        "store_embedding": write.store_embedding,
                        "metadata": write.metadata,
                    }),
                )
                .await;
        }
        None => {
            trace
                .record(
                    modules,
                    "cache_write_skipped",
                    "cache",
                    Some("Response not eligible for cache write".to_string()),
                    serde_json::json!({
                        "route": route_for_cache,
                        "memory_context_used": memory_context_used,
                        "used_tools": used_tools,
                        "skill_cacheable": skill_cacheable,
                        "response_was_fallback": response_was_fallback,
                    }),
                )
                .await;
        }
    }

    let pending_persistence = PendingTextPersistence {
        session_id,
        user_id: uid,
        user_id_text: user_id.to_string(),
        query_text: text.to_string(),
        response_text: response.clone(),
        channel_id: channel_id.to_string(),
        cache_write,
    };

    if !defer_persistence {
        persist_text_interaction(
            modules,
            pending_persistence.clone(),
            memory_config,
            event_bus,
            Some(&mut trace),
        )
        .await;
    }
    match cancellable_stage(turn, "observe_assistant_affect_for_turn", async {
        Ok(observe_assistant_affect_for_turn(
            modules,
            &mut trace,
            channel_id,
            user_id,
            &response,
            route_for_cache,
            used_tools,
            response_was_fallback,
            debug_output,
        )
        .await)
    })
    .await?
    {
        Some(value) => value,
        None => return Ok(TextResult::interrupted()),
    };
    trace
        .record(
            modules,
            "turn_completed",
            "pipeline",
            Some("Turn completed".to_string()),
            serde_json::json!({
                "route": route_for_cache,
                "streamed": streamed,
                "audio_streamed": audio_streamed,
                "used_tools": used_tools,
                "response_chars": response.chars().count(),
                "defer_persistence": defer_persistence,
            }),
        )
        .await;
    let planned_expression = match cancellable_stage(turn, "plan_expression_for_response", async {
        Ok(plan_expression_for_response(
            modules,
            &mut trace,
            channel,
            channel_id,
            user_id,
            &response,
            route_for_cache,
            used_tools,
            response_was_fallback,
            false,
            text,
            prior_message_count,
            memory_context_used,
            memory_config,
            debug_output,
        )
        .await)
    })
    .await?
    {
        Some(value) => value,
        None => return Ok(TextResult::interrupted()),
    };
    let tts_trace = TtsTraceContext::from_agent_trace(modules, &trace);

    Ok(TextResult {
        text: response,
        streamed,
        audio_streamed,
        interrupted: false,
        pending_persistence: defer_persistence.then_some(pending_persistence),
        expression_plan: Some(planned_expression.plan),
        expression_input: Some(planned_expression.input),
        tts_trace: Some(tts_trace),
    })
}

async fn observe_user_affect_for_turn(
    modules: &ModuleRegistry,
    trace: &mut AgentTrace,
    channel_id: &str,
    user_id: &str,
    user_text: &str,
    debug_output: bool,
) {
    let observation = modules
        .expression
        .observe_user_input(AffectUserInput {
            channel_id,
            user_id,
            user_text,
        })
        .await;
    let payload = serde_json::json!({
        "affect": observation.snapshot.affect,
        "social": observation.snapshot.social,
        "resource": observation.snapshot.resource,
        "dominant_causes": observation.snapshot.dominant_causes,
        "appraisals": observation.appraisals,
        "active_event_count": observation.active_event_count,
    });
    debug_component_json(
        debug_output,
        "affect",
        "user_input",
        &payload,
        Some((channel_id, user_id)),
    );
    trace
        .record(
            modules,
            "affect_user_input",
            "affect",
            Some("User input adjusted affect state".to_string()),
            payload,
        )
        .await;
}

async fn observe_assistant_affect_for_turn(
    modules: &ModuleRegistry,
    trace: &mut AgentTrace,
    channel_id: &str,
    user_id: &str,
    response: &str,
    route: &str,
    used_tools: bool,
    response_was_fallback: bool,
    debug_output: bool,
) {
    let observation = modules
        .expression
        .observe_assistant_response(AffectAssistantResponse {
            channel_id,
            user_id,
            response_text: response,
            route,
            used_tools,
            response_was_fallback,
        })
        .await;
    let payload = serde_json::json!({
        "affect": observation.snapshot.affect,
        "social": observation.snapshot.social,
        "resource": observation.snapshot.resource,
        "dominant_causes": observation.snapshot.dominant_causes,
        "appraisals": observation.appraisals,
        "active_event_count": observation.active_event_count,
        "route": route,
        "used_tools": used_tools,
        "response_was_fallback": response_was_fallback,
    });
    debug_component_json(
        debug_output,
        "affect",
        "assistant_response",
        &payload,
        Some((channel_id, user_id)),
    );
    trace
        .record(
            modules,
            "affect_state",
            "affect",
            Some("Assistant response adjusted affect state".to_string()),
            payload,
        )
        .await;
}

async fn plan_expression_for_response(
    modules: &ModuleRegistry,
    trace: &mut AgentTrace,
    channel: &dyn ChannelAdapter,
    channel_id: &str,
    user_id: &str,
    response: &str,
    route: &str,
    used_tools: bool,
    response_was_fallback: bool,
    cache_hit: bool,
    user_text: &str,
    prior_message_count: usize,
    memory_context_used: bool,
    memory_config: &MemorySection,
    debug_output: bool,
) -> PlannedExpression {
    let capabilities = channel.capabilities();
    let expression_capabilities = if capabilities.hardware_control {
        Some(negotiate_expression_capabilities(channel).await)
    } else {
        None
    };
    let memory_context = retrieve_runtime_memory_context_pack(
        stable_user_uuid(user_id),
        modules,
        memory_config,
        user_text,
    )
    .await;
    if let Some(memory_context) = &memory_context
        && !memory_context.recall_events.is_empty()
    {
        let observation = modules
            .expression
            .observe_memory_recall(channel_id, user_id, &memory_context.recall_events)
            .await;
        debug_component_json(
            debug_output,
            "affect",
            "memory_recall",
            &serde_json::json!({
                "recall_events": &memory_context.recall_events,
                "affect": observation.snapshot.affect,
                "social": observation.snapshot.social,
                "resource": observation.snapshot.resource,
                "dominant_causes": observation.snapshot.dominant_causes,
                "appraisals": &observation.appraisals,
                "active_event_count": observation.active_event_count,
            }),
            Some((channel_id, user_id)),
        );
        trace
            .record(
                modules,
                "memory_recall_affect",
                "affect",
                Some("Memory recall softly biased affect state".to_string()),
                serde_json::json!({
                    "recall_events": &memory_context.recall_events,
                    "affect": observation.snapshot.affect,
                    "social": observation.snapshot.social,
                    "resource": observation.snapshot.resource,
                    "dominant_causes": observation.snapshot.dominant_causes,
                    "appraisals": observation.appraisals,
                    "active_event_count": observation.active_event_count,
                }),
            )
            .await;
    }
    let affect = modules
        .expression
        .affect_snapshot(channel_id, user_id)
        .await;
    let dialogue_policy_input = DialoguePolicyInput {
        route,
        used_tools,
        response_was_fallback,
        cache_hit,
        user_text,
        memory_context_used,
    };
    let dialogue_policy = dialogue_policy::decide(
        dialogue_policy_input.clone(),
        modules.expression.dialogue_policy(),
    );
    let conversation_context =
        dialogue_policy.conversation_context(&dialogue_policy_input, prior_message_count);
    let constraints = dialogue_policy.constraints();
    let dialogue_act = dialogue_policy.dialogue_act.clone();
    let content_tone = dialogue_policy.content_tone.clone();
    let user_state = expression_user_state_from_affect(&affect);
    let device_context = expression_device_context(&capabilities, expression_capabilities.as_ref());
    let tts_capabilities = if capabilities.voice_output {
        Some(modules.tts.tts_capabilities().await)
    } else {
        None
    };
    let safety_context = expression_safety_context(
        &capabilities,
        &content_tone,
        expression_capabilities.as_ref(),
    );
    let expression_input = ExpressionInput {
        response_text: response.to_string(),
        route: Some(route.to_string()),
        channel_id: channel_id.to_string(),
        user_id: user_id.to_string(),
        dialogue_act: dialogue_act.clone(),
        content_tone: content_tone.clone(),
        user_state: user_state.clone(),
        device_context: device_context.clone(),
        safety_context: safety_context.clone(),
        conversation_context: conversation_context.clone(),
        constraints: constraints.clone(),
        memory_context: memory_context.clone(),
        voice_output: capabilities.voice_output,
        hardware_control: capabilities.hardware_control,
        tts_capabilities: tts_capabilities.clone(),
        expression_capabilities: expression_capabilities.clone(),
    };
    let plan = modules.expression.plan(expression_input.clone()).await;
    debug_component_json(
        debug_output,
        "expression",
        "plan",
        &serde_json::json!({
            "plan_id": plan.id,
            "intent": &plan.intent,
            "social_function": &plan.social_function,
            "style": &plan.style,
            "intensity": plan.intensity,
            "priority": plan.priority,
            "safety_class": plan.safety_class,
            "timeline_actions": plan.timeline.len(),
            "route": route,
            "dialogue_act": &dialogue_act,
            "content_tone": &content_tone,
            "conversation_context": &conversation_context,
            "constraints": &constraints,
            "user_state": &user_state,
            "safety_context": &safety_context,
            "affect": affect.affect,
            "social": affect.social,
            "resource": affect.resource,
            "dominant_causes": affect.dominant_causes,
            "expression_capabilities": &expression_capabilities,
        }),
        Some((channel_id, user_id)),
    );
    trace
        .record(
            modules,
            "expression_plan",
            "expression",
            Some(plan.intent.clone()),
            serde_json::json!({
                "plan_id": plan.id,
                "intent": plan.intent,
                "social_function": plan.social_function,
                "style": plan.style,
                "intensity": plan.intensity,
                "stance": plan.stance,
                "input": &expression_input,
                "plan": plan,
                "timeline_actions": plan.timeline.len(),
                "route": route,
                "dialogue_act": dialogue_act,
                "content_tone": content_tone,
                "conversation_context": conversation_context,
                "constraints": constraints,
                "user_state": user_state,
                "device_context": device_context,
                "safety_context": safety_context,
                "tts_capabilities": tts_capabilities,
                "affect": affect.affect,
                "social": affect.social,
                "resource": affect.resource,
                "dominant_causes": affect.dominant_causes,
                "expression_capabilities": expression_capabilities,
            }),
        )
        .await;
    if let Some(speech) = plan.speech.as_ref() {
        let request = TtsRequest::with_speech(response, speech);
        let adjustments = modules.tts.provider_adjustments(&request).await;
        trace
            .record(
                modules,
                "tts_provider_adjustments",
                "tts",
                Some("TTS provider expression adjustments".to_string()),
                serde_json::json!({
                    "plan_id": plan.id,
                    "request": request,
                    "adjustments": adjustments,
                }),
            )
            .await;
    }
    PlannedExpression {
        plan,
        input: expression_input,
    }
}

fn expression_user_state_from_affect(
    affect: &crate::expression::AffectSnapshot,
) -> ExpressionUserState {
    ExpressionUserState {
        frustration: ((-affect.affect.valence).max(0.0)
            + (affect.affect.arousal - 0.55).max(0.0) * 0.5)
            .clamp(0.0, 1.0),
        engagement: affect.social.engagement.clamp(0.0, 1.0),
    }
}

fn expression_device_context(
    capabilities: &crate::channels::adapter::ChannelCapabilities,
    expression_capabilities: Option<&crate::expression::ExpressionCapabilitySnapshot>,
) -> ExpressionDeviceContext {
    let mut context = ExpressionDeviceContext {
        supports_motion: capabilities.hardware_control,
        supports_face: capabilities.rich_messages,
        supports_light: false,
        supports_haptics: false,
        battery_level: None,
        battery_saver: false,
        thermal_limit: false,
    };
    if let Some(expression_capabilities) = expression_capabilities {
        context.supports_motion &= expression_capabilities
            .current_state
            .motion_adapter_registered
            || expression_capabilities
                .capabilities
                .values()
                .any(|capability| capability.supported);
        context.battery_saver = expression_capabilities.current_state.battery_saver;
        context.thermal_limit = expression_capabilities.current_state.thermal_limit;
        context.battery_level = expression_capabilities.current_state.battery_level;
    }
    context
}

fn expression_safety_context(
    capabilities: &crate::channels::adapter::ChannelCapabilities,
    content_tone: &str,
    expression_capabilities: Option<&crate::expression::ExpressionCapabilitySnapshot>,
) -> ExpressionSafetyContext {
    ExpressionSafetyContext {
        motion_allowed: capabilities.hardware_control,
        active_task_serious: matches!(content_tone, "serious"),
        minimal_expression: false,
        night_mode: false,
        volume_limit: None,
        max_expression_intensity: None,
        max_motion_intensity: expression_capabilities
            .and_then(|snapshot| {
                snapshot
                    .capabilities
                    .values()
                    .filter_map(|capability| capability.max_intensity)
                    .min_by(|left, right| left.total_cmp(right))
            })
            .map(|limit| limit.clamp(0.0, 1.0)),
    }
}

fn transcript_correction_llm(
    modules: &ModuleRegistry,
    policy: &crate::config::TranscriptCorrectionPolicySection,
) -> Option<Arc<dyn LlmAdapter>> {
    match policy.provider.trim() {
        "transcript_correction_llm" => modules.transcript_correction_llm.clone(),
        "minor_llm" => Some(modules.minor_llm.clone()),
        "major_llm" => Some(modules.major_llm.clone()),
        "default_llm" => {
            if let Some(llm) = &modules.transcript_correction_llm {
                Some(llm.clone())
            } else if modules.minor_llm.name() != "noop_llm" {
                Some(modules.minor_llm.clone())
            } else if modules.major_llm.name() != "noop_llm" {
                Some(modules.major_llm.clone())
            } else {
                None
            }
        }
        "none" => None,
        other => {
            tracing::warn!(
                provider = other,
                "unknown transcript correction provider; falling back to default_llm"
            );
            if let Some(llm) = &modules.transcript_correction_llm {
                Some(llm.clone())
            } else if modules.minor_llm.name() != "noop_llm" {
                Some(modules.minor_llm.clone())
            } else if modules.major_llm.name() != "noop_llm" {
                Some(modules.major_llm.clone())
            } else {
                None
            }
        }
    }
}

async fn build_transcript_correction_context(
    modules: &ModuleRegistry,
    sensors: &SensorRegistry,
    channel_id: &str,
    user_id: &str,
    conversation_config: &ConversationSection,
) -> TranscriptCorrectionContext {
    let mut context = TranscriptCorrectionContext::new(channel_id, user_id);

    if let Ok(skills) = modules.skills.list_skills().await {
        context.active_skill_names = skills
            .into_iter()
            .map(|skill| {
                if skill.display_name.trim().is_empty() {
                    skill.name
                } else {
                    format!("{} ({})", skill.display_name, skill.name)
                }
            })
            .take(24)
            .collect();
    }

    if let Ok(sensor_summaries) = sensors.list_sensors().await {
        let mut known_device_names = Vec::new();
        let mut room_names = Vec::new();
        for sensor in sensor_summaries.into_iter().take(48) {
            push_unique(&mut known_device_names, sensor.display_name.clone(), 48);
            push_unique(&mut known_device_names, sensor.sensor_id.clone(), 48);
            for room in inferred_room_names(&sensor.sensor_id, &sensor.display_name) {
                push_unique(&mut room_names, room, 24);
            }
        }
        context.known_device_names = known_device_names;
        context.room_names = room_names;
    }

    if let Ok(session_id) = modules
        .conversation
        .get_or_create_active_session(channel_id, user_id)
        .await
        && let Ok(history) = modules
            .conversation
            .get_history(
                session_id,
                conversation_config.max_context_messages.max(1) as u64,
            )
            .await
    {
        context.last_assistant_question = history
            .iter()
            .rev()
            .find(|message| {
                message.role == "assistant" && {
                    let trimmed = message.content.trim();
                    trimmed.ends_with('?') || trimmed.ends_with('？')
                }
            })
            .map(|message| preview_for_prompt(&message.content, 280));
    }

    context
}

fn push_unique(values: &mut Vec<String>, value: String, max_len: usize) {
    let value = value.trim();
    if value.is_empty() || values.len() >= max_len {
        return;
    }
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn inferred_room_names(sensor_id: &str, display_name: &str) -> Vec<String> {
    let mut rooms = Vec::new();
    for text in [sensor_id, display_name] {
        let lower = text.to_ascii_lowercase();
        for marker in [
            "kitchen",
            "living room",
            "bedroom",
            "bathroom",
            "office",
            "study",
            "hallway",
            "garage",
            "balcony",
        ] {
            if lower.contains(marker) {
                push_unique(&mut rooms, marker.to_string(), 12);
            }
        }
    }
    rooms
}

async fn handle_transcript_correction_confirmation(
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    clarification: &str,
    correction: &TranscriptCorrectionResult,
    debug_output: bool,
    turn: Option<&VoiceTurn>,
) {
    match modules
        .conversation
        .get_or_create_active_session(channel_id, user_id)
        .await
    {
        Ok(session_id) => {
            let mut trace = AgentTrace::new(session_id);
            trace
                .record(
                    modules,
                    "stt_correction",
                    "stt",
                    Some("STT transcript correction requires confirmation".to_string()),
                    transcript_correction_trace_payload(correction, &correction.raw_text),
                )
                .await;
            trace
                .record(
                    modules,
                    "correction_confirmation_requested",
                    "pipeline",
                    Some("Ambiguous STT correction was paused for user confirmation".to_string()),
                    serde_json::json!({
                        "clarification_text": clarification,
                        "raw_text": correction.raw_text,
                        "corrected_text": correction.corrected_text,
                        "risk": correction.risk,
                    }),
                )
                .await;
            let _ = modules
                .conversation
                .append_message(
                    session_id,
                    ChatMessage {
                        role: "user".into(),
                        content: correction.raw_text.clone(),
                        name: Some(user_id.to_string()),
                        ..ChatMessage::default()
                    },
                )
                .await;
            let _ = modules
                .conversation
                .append_message(
                    session_id,
                    ChatMessage {
                        role: "assistant".into(),
                        content: clarification.to_string(),
                        name: None,
                        ..ChatMessage::default()
                    },
                )
                .await;
        }
        Err(error) => {
            tracing::debug!(error = ?error, "failed to create session for STT correction confirmation");
        }
    }

    debug_component_text(
        debug_output,
        "stt",
        "clarification",
        clarification,
        Some((channel_id, user_id)),
    );
    if let Err(error) = cancellable_stage(
        turn,
        "clarification_delivery",
        channel.send_message(target, clarification),
    )
    .await
    {
        log_channel_delivery_failure(channel, error).await;
        return;
    }
    if channel.capabilities().voice_output {
        deliver_tts_audio(
            modules,
            channel,
            target,
            channel_id,
            user_id,
            clarification,
            None,
            None,
            None,
            None,
            debug_output,
            turn,
        )
        .await;
    }
}

fn transcript_correction_trace_payload(
    correction: &TranscriptCorrectionResult,
    text_used: &str,
) -> serde_json::Value {
    serde_json::json!({
        "raw_text": correction.raw_text,
        "corrected_text": correction.corrected_text,
        "text_used": text_used,
        "confidence": correction.confidence,
        "changed": correction.changed,
        "corrections": correction.corrections,
        "ambiguities": correction.ambiguities,
        "requires_confirmation": correction.requires_confirmation,
        "clarification_text": correction.clarification_text,
        "risk": correction.risk,
        "status": correction.status,
        "provider": correction.provider,
        "stt": correction.stt,
    })
}

fn transcript_correction_debug_payload(
    correction: &TranscriptCorrectionResult,
    text_used: &str,
) -> serde_json::Value {
    serde_json::json!({
        "raw_text": correction.raw_text,
        "corrected_text": correction.corrected_text,
        "text_used": text_used,
        "confidence": correction.confidence,
        "changed": correction.changed,
        "corrections": correction.corrections,
        "ambiguities": correction.ambiguities,
        "requires_confirmation": correction.requires_confirmation,
        "risk": correction.risk,
        "status": correction.status,
        "provider": correction.provider,
        "stt_confidence": correction.stt.as_ref().and_then(|stt| stt.confidence),
        "stt_language": correction.stt.as_ref().and_then(|stt| stt.language.as_deref()),
    })
}

async fn execute_expression_and_trace(
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    plan: &ExpressionPlan,
    phase: Option<ExpressionExecutionPhase>,
    debug_output: bool,
    channel_id: &str,
    user_id: &str,
    turn: Option<&VoiceTurn>,
) {
    let execution = cancellable_stage(turn, "expression", async {
        Ok(match phase {
            Some(phase) => execute_expression_plan_phase(channel, target, plan, phase).await,
            None => execute_expression_plan(channel, target, plan).await,
        })
    })
    .await
    .ok()
    .flatten();
    debug_component_json(
        debug_output,
        "expression",
        phase
            .map(ExpressionExecutionPhase::label)
            .unwrap_or("execution"),
        &serde_json::json!({
            "plan_id": plan.id,
            "timeline_actions": plan.timeline.len(),
            "execution": execution,
        }),
        Some((channel_id, user_id)),
    );
}

fn build_llm_messages(
    base_system_messages: &[ChatMessage],
    history: &[ChatMessage],
    system_prompt: Option<String>,
    text: &str,
    user_id: &str,
) -> Vec<ChatMessage> {
    let mut messages = base_system_messages.to_vec();
    if let Some(sp) = system_prompt {
        messages.push(ChatMessage {
            role: "system".into(),
            content: sp,
            name: None,
            ..ChatMessage::default()
        });
    }
    append_history_with_interrupted_context(&mut messages, history);
    messages.push(ChatMessage {
        role: "user".into(),
        content: text.to_string(),
        name: Some(user_id.to_string()),
        ..ChatMessage::default()
    });
    messages
}

fn append_history_with_interrupted_context(
    messages: &mut Vec<ChatMessage>,
    history: &[ChatMessage],
) {
    if let Some(previous) = unanswered_user_history_tail(history) {
        messages.push(ChatMessage::new(
            "system",
            interrupted_user_context_prompt(&previous.content),
        ));
    }
    messages.extend_from_slice(history);
}

fn unanswered_user_history_tail(history: &[ChatMessage]) -> Option<&ChatMessage> {
    history.last().filter(|message| message.role == "user")
}

fn interrupted_user_context_prompt(previous_user_message: &str) -> String {
    format!(
        "The prior voice turn was interrupted before the assistant could answer. \
The last user message in conversation history is preserved only as context, not as a completed exchange. \
Prior interrupted user message: \"{}\". \
Use the newest user message as the primary instruction. Refer to the interrupted message only when it helps answer the newest request.",
        preview_for_prompt(previous_user_message, 360)
    )
}

fn allow_streaming_for_tools(
    tools: &[ToolDefinition],
    allow_streaming: bool,
    channel: &dyn ChannelAdapter,
) -> bool {
    allow_streaming && tools.is_empty() && channel.capabilities().streaming
}

#[allow(clippy::too_many_arguments)]
async fn build_sentence_speech_context(
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    channel_id: &str,
    user_id: &str,
    route: &'static str,
    used_tools: bool,
    response_was_fallback: bool,
    cache_hit: bool,
    user_text: &str,
    prior_message_count: usize,
    memory_context_used: bool,
    memory_context: Option<MemoryContextPack>,
    enabled: bool,
) -> Option<SentenceSpeechContext> {
    if !enabled || !channel.capabilities().voice_output {
        return None;
    }

    let affect = modules
        .expression
        .affect_snapshot(channel_id, user_id)
        .await;
    let dialogue_policy_input = DialoguePolicyInput {
        route,
        used_tools,
        response_was_fallback,
        cache_hit,
        user_text,
        memory_context_used,
    };
    let dialogue_policy = dialogue_policy::decide(
        dialogue_policy_input.clone(),
        modules.expression.dialogue_policy(),
    );
    let capabilities = channel.capabilities();
    let conversation_context =
        dialogue_policy.conversation_context(&dialogue_policy_input, prior_message_count);
    let constraints = dialogue_policy.constraints();
    let content_tone = dialogue_policy.content_tone.clone();
    Some(SentenceSpeechContext {
        route: Some(route.to_string()),
        channel_id: channel_id.to_string(),
        user_id: user_id.to_string(),
        dialogue_act: dialogue_policy.dialogue_act,
        content_tone: content_tone.clone(),
        user_state: expression_user_state_from_affect(&affect),
        device_context: expression_device_context(&capabilities, None),
        safety_context: expression_safety_context(&capabilities, &content_tone, None),
        conversation_context,
        constraints,
        memory_context,
        tts_capabilities: Some(modules.tts.tts_capabilities().await),
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute_llm_response(
    modules: &ModuleRegistry,
    llm: &dyn LlmAdapter,
    messages: Vec<ChatMessage>,
    tool_selection: ToolSelection,
    user_uuid: Uuid,
    session_id: Uuid,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    event_bus: &EventBus,
    route: &'static str,
    audio_response: bool,
    allow_streaming: bool,
    tools_config: &ToolsSection,
    debug_output: bool,
    trace: &mut AgentTrace,
    turn: Option<&VoiceTurn>,
    sentence_speech_context: Option<SentenceSpeechContext>,
) -> anyhow::Result<LlmExecutionOutcome> {
    let available_tool_names = tool_selection
        .available_tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    let selected_tool_names = tool_selection
        .selected_tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();

    if !tool_selection.available_tools.is_empty()
        || !tool_selection.selected_tools.is_empty()
        || !tool_selection.preferred_tool_names.is_empty()
    {
        debug_component_tool_selection(
            debug_output,
            &tool_selection.available_tools,
            &tool_selection.selected_tools,
            &tool_selection.preferred_tool_names,
            Some((channel_id, user_id)),
        );
        trace
            .record(
                modules,
                "tool_selection",
                "tools",
                Some(route.to_string()),
                serde_json::json!({
                    "route": route,
                    "available_tools": available_tool_names.clone(),
                    "selected_tools": selected_tool_names.clone(),
                    "preferred_tools": tool_selection.preferred_tool_names.clone(),
                }),
            )
            .await;
    }

    if !tool_selection.selected_tools.is_empty() {
        trace
            .record(
                modules,
                "llm_call",
                "llm",
                Some(route.to_string()),
                serde_json::json!({
                    "route": route,
                    "mode": "tool_loop",
                    "selected_tools": selected_tool_names.clone(),
                    "messages": messages.len(),
                }),
            )
            .await;
        let outcome = run_llm_tool_loop(
            modules,
            llm,
            messages,
            tool_selection.selected_tools,
            user_uuid,
            Some(session_id),
            channel_id,
            user_id,
            route,
            tools_config,
            debug_output,
            trace,
            turn,
        )
        .await?;
        debug_component_text(
            debug_output,
            "llm",
            "tool_loop_result",
            &outcome.text,
            Some((channel_id, user_id)),
        );
        trace
            .record(
                modules,
                "llm_result",
                "llm",
                Some("Tool loop completed".to_string()),
                serde_json::json!({
                    "route": route,
                    "mode": "tool_loop",
                    "used_tools": outcome.used_tools,
                    "response_chars": outcome.text.chars().count(),
                }),
            )
            .await;
        return Ok(LlmExecutionOutcome {
            text: outcome.text,
            streamed: false,
            audio_streamed: false,
            used_tools: outcome.used_tools,
        });
    }

    if allow_streaming && channel.capabilities().streaming {
        debug_component_llm_call(
            debug_output,
            "llm",
            "stream_call",
            route,
            &messages,
            Some((channel_id, user_id)),
        );
        trace
            .record(
                modules,
                "llm_call",
                "llm",
                Some(route.to_string()),
                serde_json::json!({
                    "route": route,
                    "mode": "stream",
                    "messages": messages.len(),
                    "audio_response": audio_response,
                }),
            )
            .await;
        let (full, audio_streamed) = cancellable_stage(turn, "response_stream", async {
            Ok(if audio_response {
                stream_llm_response_with_sentence_tts(
                    modules,
                    llm,
                    channel,
                    target,
                    channel_id,
                    user_id,
                    event_bus,
                    &messages,
                    sentence_speech_context,
                    Some(TtsTraceContext::from_agent_trace(modules, trace)),
                    debug_output,
                    turn,
                )
                .await?
            } else {
                (
                    stream_llm_response(
                        modules,
                        llm,
                        channel,
                        target,
                        channel_id,
                        user_id,
                        event_bus,
                        &messages,
                        debug_output,
                        turn,
                    )
                    .await?,
                    false,
                )
            })
        })
        .await?
        .unwrap_or_default();
        debug_component_text(
            debug_output,
            "llm",
            "stream_result",
            &full,
            Some((channel_id, user_id)),
        );
        trace
            .record(
                modules,
                "llm_result",
                "llm",
                Some("Streaming response completed".to_string()),
                serde_json::json!({
                    "route": route,
                    "mode": "stream",
                    "response_chars": full.chars().count(),
                    "audio_streamed": audio_streamed,
                }),
            )
            .await;
        return Ok(LlmExecutionOutcome {
            text: full,
            streamed: true,
            audio_streamed,
            used_tools: false,
        });
    }

    debug_component_llm_call(
        debug_output,
        "llm",
        "call",
        route,
        &messages,
        Some((channel_id, user_id)),
    );
    trace
        .record(
            modules,
            "llm_call",
            "llm",
            Some(route.to_string()),
            serde_json::json!({
                "route": route,
                "mode": "chat",
                "messages": messages.len(),
            }),
        )
        .await;
    let full = match cancellable_stage(turn, "llm", llm.chat(&messages)).await? {
        Some(full) => full,
        None => String::new(),
    };
    if !turn_is_interrupted(turn) {
        debug_component_text(
            debug_output,
            "llm",
            "result",
            &full,
            Some((channel_id, user_id)),
        );
    }
    trace
        .record(
            modules,
            "llm_result",
            "llm",
            Some("Chat response completed".to_string()),
            serde_json::json!({
                "route": route,
                "mode": "chat",
                "response_chars": full.chars().count(),
                "interrupted": turn_is_interrupted(turn),
            }),
        )
        .await;
    Ok(LlmExecutionOutcome {
        text: full,
        streamed: false,
        audio_streamed: false,
        used_tools: false,
    })
}

pub fn escalation_reason(response: &str) -> Option<&str> {
    let trimmed = response.trim_start();
    let reason = trimmed.strip_prefix("[ESCALATE]")?.trim();
    Some(if reason.is_empty() {
        "minor model requested escalation"
    } else {
        reason
    })
}

pub fn cached_response_is_safe_direct(cached: &CachedResponse) -> bool {
    let metadata = &cached.metadata;
    let policy = metadata
        .get("policy")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();

    metadata.get("schema").and_then(serde_json::Value::as_str) == Some(CACHE_METADATA_SCHEMA)
        && matches!(policy, "private_exact" | "shared_exact")
        && metadata
            .get("safe_direct_return")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && metadata
            .get("memory_independent")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && metadata
            .get("tool_free")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && metadata
            .get("dynamic_free")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && metadata.get("status").and_then(serde_json::Value::as_str) == Some(CACHE_STATUS_ACTIVE)
}

pub fn cache_write_for_response(
    cache_config: &CacheSection,
    cache_policy: &CachePolicyResourceSection,
    query_text: &str,
    response_text: &str,
    context: CacheContext,
) -> Option<CacheWrite> {
    if !cache_config.enabled || !cache_config.exact_match_enabled {
        return None;
    }
    if context.response_was_fallback
        || context.used_tools
        || context.memory_context_used
        || response_text.trim().is_empty()
        || matches!(context.route, "router_cache" | "major_llm_escalated")
    {
        return None;
    }
    if matches!(context.skill_cacheable, Some(false)) {
        return None;
    }
    if query_matches_keywords(query_text, &cache_policy.dynamic_query_keywords)
        || query_matches_keywords(query_text, &cache_policy.memory_query_keywords)
        || query_matches_keywords(query_text, &cache_policy.action_query_keywords)
    {
        return None;
    }

    let ttl_seconds = if context.skill_cacheable == Some(true) {
        cache_config.skill_cache_ttl_seconds
    } else {
        cache_config.default_ttl_seconds
    };
    if ttl_seconds == 0 {
        return None;
    }

    Some(CacheWrite {
        scope: DEFAULT_CACHE_SCOPE.into(),
        metadata: serde_json::json!({
            "schema": CACHE_METADATA_SCHEMA,
            "policy": "private_exact",
            "status": CACHE_STATUS_PENDING_INSIGHTS_REVIEW,
            "safe_direct_return": false,
            "memory_independent": true,
            "tool_free": true,
            "dynamic_free": true,
            "route": context.route,
            "language": guess_text_language(query_text),
            "created_by": "pipeline",
            "review_required_by": "insights",
        }),
        ttl_seconds,
        store_embedding: cache_config.semantic_match_enabled,
    })
}

fn runtime_context_has_user_memory(context: &RuntimePromptContext) -> bool {
    !context.core_memories.is_empty()
        || !context.user_preferences.is_empty()
        || context
            .memory_context
            .as_ref()
            .is_some_and(|memory_context| !memory_context.is_empty())
}

fn memory_policy_enabled(memory: &MemorySection) -> bool {
    if !memory.enabled {
        return false;
    }
    let Some(paused_until) = memory.paused_until.as_deref().map(str::trim) else {
        return true;
    };
    if paused_until.is_empty() {
        return true;
    }
    chrono::DateTime::parse_from_rfc3339(paused_until)
        .map(|until| until.with_timezone(&chrono::Utc) <= chrono::Utc::now())
        .unwrap_or(true)
}

async fn semantic_cache_candidates(
    modules: &ModuleRegistry,
    user_id: Uuid,
    query_text: &str,
    debug_output: bool,
) -> Vec<SemanticCacheCandidate> {
    if modules.embedding.name() == "noop_embedding" {
        return Vec::new();
    }

    let embeddings = match modules.embedding.embed(&[query_text.to_string()]).await {
        Ok(embeddings) => embeddings,
        Err(error) => {
            tracing::debug!(error = ?error, "semantic cache embedding lookup failed");
            return Vec::new();
        }
    };
    let Some(embedding) = embeddings.first() else {
        return Vec::new();
    };
    let candidates = match modules
        .cache
        .get_semantic_candidates(Some(user_id), embedding, 3)
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::debug!(error = ?error, "semantic cache candidate lookup failed");
            Vec::new()
        }
    };
    if candidates.is_empty() {
        return Vec::new();
    }

    if debug_output {
        tracing::info!(
            target: "alia_server::debug_output",
            component = "cache",
            event = "semantic_candidates",
            candidates = candidates.len(),
            "component event"
        );
    }

    candidates
}

fn semantic_cache_reference_prompt(
    candidates: &[SemanticCacheCandidate],
    debug_output: bool,
) -> Option<(String, usize)> {
    let candidates = candidates
        .iter()
        .filter(|candidate| semantic_candidate_is_safe_reference(candidate))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return None;
    }

    if debug_output {
        tracing::info!(
            target: "alia_server::debug_output",
            component = "cache",
            event = "semantic_references",
            candidates = candidates.len(),
            "component event"
        );
    }

    let references = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            format!(
                "{}. similarity={:.3}: {}",
                index + 1,
                candidate.similarity,
                preview_for_prompt(&candidate.response_text, 360)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let candidate_count = candidates.len();
    Some((
        format!(
            "Internal cache references. These are semantically similar previous answers, not authoritative facts. Reuse or adapt them only if they clearly answer the current user request; otherwise ignore them. Do not mention the cache.\n\n{}",
            references
        ),
        candidate_count,
    ))
}

fn semantic_candidate_is_safe_direct(candidate: &SemanticCacheCandidate) -> bool {
    semantic_candidate_is_safe_reference(candidate)
}

fn semantic_candidate_is_safe_reference(candidate: &SemanticCacheCandidate) -> bool {
    cached_response_is_safe_direct(&CachedResponse {
        response_text: candidate.response_text.clone(),
        metadata: candidate.metadata.clone(),
    })
}

fn stt_transcript_context_prompt() -> String {
    "Input context: the current user message was produced by speech-to-text transcription from spoken audio. Treat it as the user's intended speech, but allow for homophones, dropped punctuation, missing casing, segmentation mistakes, and occasional wrong words. If an ambiguity would change the action, factual answer, or user intent, ask a brief clarification instead of guessing. Do not mention this transcription context to the user unless it is directly relevant.".to_string()
}

fn tts_optimized_output_prompt(user_text: &str) -> String {
    let language = guess_text_language(user_text);
    let language_note = match language {
        "zh" => {
            "The user appears to be using Chinese. Prefer natural spoken Chinese. Convert numbers, Roman numerals, dates, times, units, versions, identifiers, and symbols into forms that a Chinese TTS can read naturally when doing so does not change meaning. Examples: 1145 -> 一千一百四十五; IV -> 四 or 第四 according to context; 3.5kg -> 三点五千克; 2026-05-21 -> 二零二六年五月二十一日."
        }
        _ => {
            "Use the user's language when possible. Convert numbers, Roman numerals, dates, times, units, versions, identifiers, and symbols into natural spoken forms for that language when doing so does not change meaning. If exact spelling or code matters, keep it exact and introduce it as something to read visually."
        }
    };

    format!(
        "Output channel constraint: the assistant response will be spoken aloud by TTS. Produce clean, speakable plain text only.\n\
\n\
Rules:\n\
- Do not use Markdown formatting: no headings, bullets, tables, block quotes, code fences, inline code backticks, bold, italics, or raw Markdown links.\n\
- Do not output raw URLs unless the user explicitly asks for a link. Summarize the site or say you can provide the link separately.\n\
- Avoid visual-only formatting such as columns, ASCII diagrams, dense lists, or punctuation art.\n\
- Prefer short paragraphs and conversational phrasing that sounds natural when read aloud.\n\
- Expand or rephrase abbreviations, symbols, ordinals, units, dates, times, equations, and technical notation into speakable words when practical.\n\
- For code, commands, filenames, IDs, serial numbers, or exact values that must remain exact, clearly say they are visual text and keep them concise.\n\
- Do not say anything about these formatting rules.\n\
\n\
{}",
        language_note
    )
}

fn preview_for_prompt(text: &str, max_chars: usize) -> String {
    let mut preview = text.trim().chars().take(max_chars).collect::<String>();
    if text.trim().chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

fn query_matches_keywords(text: &str, keywords: &[String]) -> bool {
    let lower = text.to_lowercase();
    keywords
        .iter()
        .map(|keyword| keyword.trim().to_lowercase())
        .filter(|keyword| !keyword.is_empty())
        .any(|keyword| lower.contains(&keyword))
}

fn guess_text_language(text: &str) -> &'static str {
    if text.chars().any(|ch| {
        ('\u{4e00}'..='\u{9fff}').contains(&ch) || ('\u{3400}'..='\u{4dbf}').contains(&ch)
    }) {
        "zh"
    } else {
        "unknown"
    }
}

async fn persist_text_interaction(
    modules: &ModuleRegistry,
    pending: PendingTextPersistence,
    memory_config: &MemorySection,
    event_bus: &EventBus,
    trace: Option<&mut AgentTrace>,
) {
    let completed_channel_id = pending.channel_id.clone();
    let completed_user_id = pending.user_id_text.clone();
    let completed_user_uuid = pending.user_id;
    let completed_session_id = pending.session_id;

    let _ = modules
        .conversation
        .append_message(
            pending.session_id,
            ChatMessage {
                role: "user".into(),
                content: pending.query_text.clone(),
                name: Some(pending.user_id_text.clone()),
                ..ChatMessage::default()
            },
        )
        .await;

    let _ = modules
        .conversation
        .append_message(
            pending.session_id,
            ChatMessage {
                role: "assistant".into(),
                content: pending.response_text.clone(),
                name: None,
                ..ChatMessage::default()
            },
        )
        .await;

    if let Some(entry) = cache_entry_for_pending(&pending) {
        let should_store_embedding = pending
            .cache_write
            .as_ref()
            .is_some_and(|cache_write| cache_write.store_embedding);
        let scope = entry.scope.clone();
        let query_text = entry.query_text.clone();
        let user_id = entry.user_id;
        if modules.cache.put(entry).await.is_ok() && should_store_embedding {
            update_cache_entry_embedding(modules, user_id, &query_text, &scope).await;
        }
    }

    let capture_trace = if !memory_policy_enabled(memory_config) {
        MemoryCaptureTrace::skipped(memory_config.capture_mode, "memory_policy_disabled")
    } else if pending.user_id.is_nil() {
        MemoryCaptureTrace::skipped(memory_config.capture_mode, "anonymous_user")
    } else {
        capture_turn_memory(modules, &pending, memory_config).await
    };
    record_memory_capture_trace(modules, &pending, capture_trace, trace).await;

    event_bus.publish(ServerEvent::ConversationTurnCompleted {
        channel_id: completed_channel_id,
        user_id: completed_user_id,
        user_uuid: completed_user_uuid,
        session_id: completed_session_id,
    });
}

async fn persist_interrupted_user_context(
    modules: &ModuleRegistry,
    pending: &PendingTextPersistence,
    turn: &VoiceTurn,
    debug_output: bool,
) {
    let interruption = turn.interruption();
    let context = InterruptedUserContext {
        session_id: pending.session_id,
        turn_id: turn.id,
        user_id: pending.user_id,
        user_id_text: pending.user_id_text.clone(),
        query_text: pending.query_text.clone(),
        channel_id: pending.channel_id.clone(),
        reason: interruption.reason,
        stage: interruption.stage,
    };
    persist_interrupted_user_context_entry(modules, context, debug_output).await;
}

async fn persist_interrupted_input_context(
    modules: &ModuleRegistry,
    session_id: Uuid,
    user_id: Uuid,
    user_id_text: &str,
    query_text: &str,
    channel_id: &str,
    turn: &VoiceTurn,
    debug_output: bool,
) {
    let interruption = turn.interruption();
    let context = InterruptedUserContext {
        session_id,
        turn_id: turn.id,
        user_id,
        user_id_text: user_id_text.to_string(),
        query_text: query_text.to_string(),
        channel_id: channel_id.to_string(),
        reason: interruption.reason,
        stage: interruption.stage,
    };
    persist_interrupted_user_context_entry(modules, context, debug_output).await;
}

async fn persist_interrupted_user_context_entry(
    modules: &ModuleRegistry,
    context: InterruptedUserContext,
    debug_output: bool,
) {
    let _ = modules
        .conversation
        .append_message(
            context.session_id,
            ChatMessage {
                role: "user".into(),
                content: context.query_text.clone(),
                name: Some(context.user_id_text.clone()),
                ..ChatMessage::default()
            },
        )
        .await;

    let event = ConversationEventEntry {
        turn_id: context.turn_id,
        // Terminal interruption shares the response audit ID; reserve the last
        // sequence so it cannot collide with normal events or TTS (10_000+).
        sequence: i32::MAX,
        event_type: "turn_interrupted".to_string(),
        actor: "pipeline".to_string(),
        summary: Some("Voice turn interrupted; preserved user context only".to_string()),
        payload: serde_json::json!({
            "channel_id": context.channel_id.as_str(),
            "user_id": context.user_id_text.as_str(),
            "user_uuid": context.user_id.to_string(),
            "reason": context.reason.as_str(),
            "stage": context.stage,
            "preserved_user_context": true,
            "assistant_response_persisted": false,
            "cache_written": false,
            "memory_capture": false,
        }),
    };
    if let Err(error) = modules
        .conversation
        .append_event(context.session_id, event)
        .await
    {
        tracing::debug!(error = ?error, "failed to append interrupted turn event");
    }

    debug_component_json(
        debug_output,
        "pipeline",
        "turn_interrupted",
        &serde_json::json!({
            "channel_id": context.channel_id.as_str(),
            "user_id": context.user_id_text.as_str(),
            "reason": context.reason.as_str(),
            "stage": context.stage,
            "preserved_user_context": true,
        }),
        Some((&context.channel_id, &context.user_id_text)),
    );
}

async fn record_memory_capture_trace(
    modules: &ModuleRegistry,
    pending: &PendingTextPersistence,
    capture_trace: MemoryCaptureTrace,
    trace: Option<&mut AgentTrace>,
) {
    let status = format!("{:?}", capture_trace.status).to_ascii_lowercase();
    let summary = match capture_trace.reason.as_deref() {
        Some(reason) => format!("Memory capture {status}: {reason}"),
        None => format!(
            "Memory capture {status}: {} stored, {} skipped",
            capture_trace.stored, capture_trace.skipped
        ),
    };
    let payload = serde_json::json!({
        "memory_capture": capture_trace,
        "context": {
            "channel_id": pending.channel_id,
            "conversation_id": pending.session_id,
            "user_id_text": pending.user_id_text,
        }
    });
    if let Some(trace) = trace {
        trace
            .record(modules, "memory_capture", "memory", Some(summary), payload)
            .await;
        return;
    }
    let event = ConversationEventEntry {
        turn_id: Uuid::new_v4(),
        sequence: 0,
        event_type: "memory_capture".to_string(),
        actor: "memory".to_string(),
        summary: Some(summary),
        payload,
    };
    if let Err(error) = modules
        .conversation
        .append_event(pending.session_id, event)
        .await
    {
        tracing::debug!(
            error = ?error,
            "failed to append standalone memory capture event"
        );
    }
}

pub fn cache_entry_for_pending(pending: &PendingTextPersistence) -> Option<CacheEntry> {
    let cache_write = pending.cache_write.as_ref()?;
    Some(CacheEntry {
        user_id: Some(pending.user_id),
        scope: cache_write.scope.clone(),
        query_text: pending.query_text.clone(),
        response_text: pending.response_text.clone(),
        metadata: cache_write.metadata.clone(),
        ttl_seconds: cache_write.ttl_seconds,
    })
}

async fn update_cache_entry_embedding(
    modules: &ModuleRegistry,
    user_id: Option<Uuid>,
    query_text: &str,
    scope: &str,
) {
    if modules.embedding.name() == "noop_embedding" {
        return;
    }
    let embeddings = match modules.embedding.embed(&[query_text.to_string()]).await {
        Ok(embeddings) => embeddings,
        Err(error) => {
            tracing::debug!(error = ?error, "cache embedding generation failed");
            return;
        }
    };
    let Some(embedding) = embeddings.first() else {
        return;
    };
    if let Err(error) = modules
        .cache
        .update_embedding(user_id, query_text, scope, embedding)
        .await
    {
        tracing::debug!(error = ?error, "cache embedding update failed");
    }
}

async fn build_base_system_messages(
    user_id: Uuid,
    modules: &ModuleRegistry,
    memory_config: &MemorySection,
    user_text: &str,
) -> anyhow::Result<(Vec<ChatMessage>, RuntimePromptContext)> {
    let mut messages = Vec::new();
    let mut assistant_name = DEFAULT_ASSISTANT_NAME.to_string();

    match modules.persona.render_prompt(user_id).await {
        Ok(rendered) => {
            assistant_name = sanitize_assistant_name(&rendered.persona.name);
            if !rendered.system_prompt.trim().is_empty() {
                messages.push(system_message(rendered.system_prompt));
            }
        }
        Err(error) => {
            tracing::warn!(user_id = %user_id, error = ?error, "failed to render persona prompt");
        }
    }

    let mut runtime_context =
        build_runtime_prompt_context(user_id, modules, memory_config, user_text).await;
    runtime_context.assistant_name = assistant_name;
    if let Some(prompt) = render_runtime_context_system_prompt(&runtime_context) {
        messages.push(system_message(prompt));
    }

    Ok((messages, runtime_context))
}

async fn build_runtime_prompt_context(
    user_id: Uuid,
    modules: &ModuleRegistry,
    memory_config: &MemorySection,
    user_text: &str,
) -> RuntimePromptContext {
    let available_skills = modules
        .skills
        .list_skills()
        .await
        .map(|skills| skills.into_iter().map(|skill| skill.display_name).collect())
        .unwrap_or_default();

    let memory_context =
        retrieve_runtime_memory_context_pack(user_id, modules, memory_config, user_text).await;
    let core_memories = runtime_memory_lines(
        memory_context.as_ref(),
        memory_config.max_injected_memory_chars,
        memory_config.sensitive_filter_enabled,
    );
    let (relationship_summary, relationship_preference_summary) =
        runtime_relationship_context(user_id, modules);

    RuntimePromptContext {
        assistant_name: DEFAULT_ASSISTANT_NAME.to_string(),
        current_time: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        core_memories,
        memory_context,
        relationship_summary,
        relationship_preference_summary,
        available_skills,
        ..RuntimePromptContext::default()
    }
}

fn runtime_relationship_context(
    user_id: Uuid,
    modules: &ModuleRegistry,
) -> (Option<String>, Option<String>) {
    if user_id.is_nil() {
        return (None, None);
    }
    let profile = modules
        .expression
        .relationship_profile(&user_id.to_string());
    if !profile.privacy.user_visible {
        return (None, None);
    }

    let summary = (!profile.summary.trim().is_empty()).then(|| profile.summary.trim().to_string());
    let preference_summary = (!profile.preference_summary.trim().is_empty())
        .then(|| profile.preference_summary.trim().to_string());
    (summary, preference_summary)
}

async fn retrieve_runtime_memories(
    user_id: Uuid,
    modules: &ModuleRegistry,
    memory_config: &MemorySection,
    user_text: &str,
) -> Vec<String> {
    let Some(pack) =
        retrieve_runtime_memory_context_pack(user_id, modules, memory_config, user_text).await
    else {
        return Vec::new();
    };

    runtime_memory_lines(
        Some(&pack),
        memory_config.max_injected_memory_chars,
        memory_config.sensitive_filter_enabled,
    )
}

fn runtime_memory_lines(
    pack: Option<&MemoryContextPack>,
    max_injected_memory_chars: usize,
    sensitive_filter_enabled: bool,
) -> Vec<String> {
    let Some(pack) = pack else {
        return Vec::new();
    };
    let nodes = pack
        .user_preferences
        .iter()
        .chain(pack.project_context.iter())
        .chain(pack.relevant_episodes.iter())
        .chain(pack.affective_guidance.iter())
        .chain(pack.boundaries.iter())
        .collect::<Vec<_>>();
    let mut total_chars = 0usize;
    let max_chars = max_injected_memory_chars.max(256);
    let mut memories = Vec::new();
    for node in nodes {
        if node.content.trim().is_empty() {
            continue;
        }
        if sensitive_filter_enabled && detect_sensitive_memory(&node.content).is_some() {
            continue;
        }
        let remaining = max_chars.saturating_sub(total_chars);
        if remaining == 0 {
            break;
        }
        let line = memory_prompt_line(&node.content, remaining.min(240));
        total_chars += line.chars().count();
        memories.push(line);
    }
    memories
}

async fn retrieve_runtime_memory_context_pack(
    user_id: Uuid,
    modules: &ModuleRegistry,
    memory_config: &MemorySection,
    user_text: &str,
) -> Option<MemoryContextPack> {
    if !memory_policy_enabled(memory_config) || user_id.is_nil() || user_text.trim().is_empty() {
        return None;
    }

    let layers = normalize_memory_layers(memory_config.retrieval_layers.clone());
    let layers = (!layers.is_empty()).then_some(layers);
    match modules
        .memory
        .retrieve_context_pack(
            user_id,
            user_text,
            RetrievalOpts {
                max_results: Some(memory_config.max_retrieval_results.max(1).min(12)),
                min_similarity: Some(memory_config.similarity_threshold.clamp(0.0, 1.0)),
                layers,
            },
        )
        .await
    {
        Ok(pack) => Some(pack),
        Err(error) => {
            tracing::warn!(user_id = %user_id, error = ?error, "failed to retrieve runtime memories");
            None
        }
    }
}

pub fn render_runtime_context_system_prompt(context: &RuntimePromptContext) -> Option<String> {
    let mut sections = Vec::new();
    let mut runtime_lines = Vec::new();

    if !context.current_time.trim().is_empty() {
        runtime_lines.push(format!("- Time: {}", context.current_time.trim()));
    }
    if !context.user_presence.trim().is_empty() {
        runtime_lines.push(format!("- User presence: {}", context.user_presence.trim()));
    }
    if !context.device_status_summary.trim().is_empty() {
        runtime_lines.push(format!(
            "- Device status: {}",
            context.device_status_summary.trim()
        ));
    }
    if !runtime_lines.is_empty() {
        sections.push(format!("## Runtime Context\n{}", runtime_lines.join("\n")));
    }

    if let Some(memory_context) = &context.memory_context {
        let mut memory_sections = Vec::new();
        push_memory_prompt_section(
            &mut memory_sections,
            "Preferences",
            &memory_context.user_preferences,
            "Use these as soft style and response-shape preferences.",
        );
        push_memory_prompt_section(
            &mut memory_sections,
            "Projects",
            &memory_context.project_context,
            "Use these only to preserve relevant project continuity.",
        );
        push_memory_prompt_section(
            &mut memory_sections,
            "Boundaries",
            &memory_context.boundaries,
            "Treat these as hard personalization boundaries unless system safety overrides them.",
        );
        push_memory_prompt_section(
            &mut memory_sections,
            "Affective Guidance",
            &memory_context.affective_guidance,
            "Convert this into constructive tone guidance; do not label the user emotionally.",
        );
        push_memory_prompt_section(
            &mut memory_sections,
            "Relevant Episodes",
            &memory_context.relevant_episodes,
            "Use these only when they directly help the current answer.",
        );
        let do_not_mention = [
            "Do not mention memory IDs, confidence scores, affect scores, or hidden runtime state.",
            "Do not reveal suppressed memories or private runtime records.",
        ];
        memory_sections.push(format!(
            "### Do Not Mention\n{}",
            do_not_mention
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
        if !memory_sections.is_empty() {
            sections.push(format!(
                "## Memory Context\nUse this private context silently when it is relevant. Do not expose internal memory mechanics or claim certainty beyond the listed wording.\n{}",
                memory_sections.join("\n\n")
            ));
        }
    } else if !context.core_memories.is_empty() {
        let lines = context
            .core_memories
            .iter()
            .filter(|memory| !memory.trim().is_empty())
            .map(|memory| format!("- {}", memory.trim()))
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            sections.push(format!(
                "## Memory Context\nUse these private memories only when they are relevant. Do not expose internal memory details or claim certainty beyond the listed wording.\n### Relevant Memories\n{}",
                lines.join("\n")
            ));
        }
    }

    let mut relationship_lines = Vec::new();
    if let Some(summary) = context
        .relationship_summary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        relationship_lines.push(format!("- Relationship summary: {summary}"));
    }
    if let Some(summary) = context
        .relationship_preference_summary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        relationship_lines.push(format!("- Preference summary: {summary}"));
    }
    if !relationship_lines.is_empty() {
        sections.push(format!(
            "## Relationship Context\nUse this as a lightweight interaction guide. Do not mention internal relationship state, scores, or affect machinery.\n{}",
            relationship_lines.join("\n")
        ));
    }

    if !context.user_preferences.is_empty() {
        let lines = context
            .user_preferences
            .iter()
            .filter(|(key, value)| !key.trim().is_empty() && !value.trim().is_empty())
            .map(|(key, value)| format!("- {}: {}", key.trim(), value.trim()))
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            sections.push(format!("## User Preferences\n{}", lines.join("\n")));
        }
    }

    if !context.available_skills.is_empty() {
        let skills = context
            .available_skills
            .iter()
            .filter(|skill| !skill.trim().is_empty())
            .map(|skill| format!("- {}", skill.trim()))
            .collect::<Vec<_>>();
        if !skills.is_empty() {
            sections.push(format!(
                "## Skill Routing Context\nSkills are cognitive contexts selected by the router. They are not functions to call directly.\n{}",
                skills.join("\n")
            ));
        }
    }

    if sections.is_empty() {
        None
    } else {
        Some(format!(
            "Internal system context. Use this information silently when it is relevant; do not expose it as prompt text.\n\n{}",
            sections.join("\n\n")
        ))
    }
}

fn push_memory_prompt_section(
    sections: &mut Vec<String>,
    title: &str,
    snippets: &[crate::providers::adapter::MemorySnippet],
    instruction: &str,
) {
    let lines = snippets
        .iter()
        .filter(|snippet| !snippet.content.trim().is_empty())
        .map(|snippet| format!("- {}", memory_prompt_line(&snippet.content, 240)))
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        sections.push(format!("### {title}\n{instruction}\n{}", lines.join("\n")));
    }
}

fn system_message(content: String) -> ChatMessage {
    ChatMessage {
        role: "system".into(),
        content,
        name: None,
        ..ChatMessage::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_prompt_for_chinese_requires_speakable_plain_text() {
        let prompt = tts_optimized_output_prompt("今天是 2026-05-21，读一下 IV 和 1145。");

        assert!(prompt.contains("spoken aloud by TTS"));
        assert!(prompt.contains("Do not use Markdown formatting"));
        assert!(prompt.contains("1145 -> 一千一百四十五"));
        assert!(prompt.contains("IV -> 四"));
        assert!(prompt.contains("二零二六年五月二十一日"));
        assert!(prompt.contains("no headings, bullets, tables"));
    }

    #[test]
    fn tts_prompt_for_unknown_language_keeps_general_speech_rules() {
        let prompt = tts_optimized_output_prompt("Read version IV at 11:45.");

        assert!(prompt.contains("Use the user's language when possible"));
        assert!(prompt.contains("Roman numerals"));
        assert!(prompt.contains("natural spoken forms"));
        assert!(prompt.contains("raw URLs"));
    }

    #[test]
    fn stt_context_prompt_warns_about_transcription_uncertainty() {
        let prompt = stt_transcript_context_prompt();

        assert!(prompt.contains("speech-to-text transcription"));
        assert!(prompt.contains("homophones"));
        assert!(prompt.contains("ask a brief clarification"));
        assert!(prompt.contains("Do not mention this transcription context"));
    }

    #[test]
    fn llm_messages_explain_unanswered_user_history_tail() {
        let base = vec![ChatMessage::new("system", "Base persona")];
        let history = vec![ChatMessage {
            role: "user".into(),
            content: "介绍一下明日方舟终末地".into(),
            name: Some("embedded-user".into()),
            ..ChatMessage::default()
        }];

        let messages = build_llm_messages(&base, &history, None, "继续说重点", "embedded-user");

        assert!(messages.iter().any(|message| {
            message.role == "system"
                && message.content.contains("prior voice turn was interrupted")
                && message.content.contains("终末地")
        }));
        assert_eq!(messages.last().unwrap().content, "继续说重点");
    }

    #[test]
    fn llm_messages_do_not_add_interruption_prompt_for_completed_pair() {
        let history = vec![
            ChatMessage::new("user", "上一问"),
            ChatMessage::new("assistant", "上一答"),
        ];

        let messages = build_llm_messages(&[], &history, None, "下一问", "embedded-user");

        assert!(
            !messages
                .iter()
                .any(|message| message.content.contains("prior voice turn was interrupted"))
        );
    }
}
