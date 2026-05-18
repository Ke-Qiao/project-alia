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
use crate::channels::adapter::{ChannelAdapter, ChannelEvent, ChannelTarget};
use crate::config::{CacheSection, ConversationSection, MemorySection, ToolsSection};
use crate::event::{EventBus, ServerEvent};
use crate::module::ModuleRegistry;
use crate::providers::adapter::{
    AudioEncoding, CACHE_METADATA_SCHEMA, CacheEntry, CachedResponse, ChatMessage,
    ConversationEventEntry, DEFAULT_CACHE_SCOPE, LlmAdapter, MemoryConversationIngest,
    MemoryConversationMessage, RetrievalOpts, RouteDecision, RouterInput, SemanticCacheCandidate,
    ToolDefinition,
};
use crate::providers::sensor::SensorRegistry;
use crate::settings::values::{bool_setting, f32_setting, u32_setting, u64_setting, usize_setting};

mod debug;
pub mod streaming;
pub mod tools;
mod turn;
use debug::*;
use streaming::*;
use tools::*;
use turn::*;

const EMPTY_RESPONSE_FALLBACK: &str = "抱歉，我刚才没有生成有效回复，请再试一次。";

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
    pub core_memories: Vec<String>,
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
    pub memory: MemorySection,
}

pub type SharedPipelinePolicies = Arc<RwLock<PipelinePolicies>>;

impl PipelinePolicies {
    pub fn new(
        conversation: ConversationSection,
        tools: ToolsSection,
        cache: CacheSection,
        memory: MemorySection,
    ) -> Self {
        Self {
            conversation,
            tools,
            cache,
            memory,
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
        if let Some(value) =
            bool_setting(updates, "core.policies.memory.conversation_ingest_enabled")
        {
            self.memory.conversation_ingest_enabled = value;
            applied.push("core.policies.memory.conversation_ingest_enabled".to_string());
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

        applied
    }
}

/// Spawn the pipeline processing loop.
///
/// The task runs until the cancel token is triggered. It subscribes to the
/// EventBus, filters for `ChannelInput` events, processes them through the
/// AI pipeline, and sends responses back via the channel registry.
pub fn spawn_pipeline(
    event_bus: &EventBus,
    modules: Arc<ModuleRegistry>,
    channels: Arc<ChannelRegistry>,
    sensors: Arc<SensorRegistry>,
    cancel_token: CancellationToken,
    debug_output: bool,
    policies: SharedPipelinePolicies,
) {
    let mut rx = event_bus.subscribe();
    let bus = event_bus.clone();
    let voice_turns = Arc::new(VoiceTurnRegistry::default());

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("pipeline loop shutting down");
                    break;
                }
                result = rx.recv() => {
                    match result {
                        Ok(ServerEvent::ChannelInput(event)) => {
                            let channel_id = channel_event_id(&event).map(ToOwned::to_owned);
                            let modules = modules.clone();
                            let channels = channels.clone();
                            let sensors = sensors.clone();
                            let bus = bus.clone();
                            let policy_snapshot = policies.read().await.clone();
                            let voice_turns = voice_turns.clone();
                            // Process each event concurrently
                            tokio::spawn(async move {
                                if let Err(e) = process_event(event, &modules, &channels, &sensors, &bus, &voice_turns, debug_output, &policy_snapshot.conversation, &policy_snapshot.tools, &policy_snapshot.cache, &policy_snapshot.memory).await {
                                    tracing::error!(error = %e, "pipeline processing failed");
                                    tracing::debug!(error = ?e, "pipeline processing failure details");
                                    if let Some(channel_id) = channel_id {
                                        tracing::warn!(
                                            channel_id = %channel_id,
                                            error = %e,
                                            "channel pipeline run failed; keeping channel enabled"
                                        );
                                    }
                                }
                            });
                        }
                        Ok(_) => {
                            // Ignore non-channel events
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(missed = n, "pipeline lagged, some events dropped");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::info!("event bus closed, pipeline stopping");
                            break;
                        }
                    }
                }
            }
        }
    });
}

/// Process a single ChannelEvent through the AI pipeline.
async fn process_event(
    event: ChannelEvent,
    modules: &ModuleRegistry,
    channels: &ChannelRegistry,
    sensors: &SensorRegistry,
    event_bus: &EventBus,
    voice_turns: &VoiceTurnRegistry,
    debug_output: bool,
    conversation_config: &ConversationSection,
    tools_config: &ToolsSection,
    cache_config: &CacheSection,
    memory_config: &MemorySection,
) -> anyhow::Result<()> {
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
            let channel = channels.get(&channel_id).await.ok_or_else(|| {
                anyhow::anyhow!("channel '{}' not found for response delivery", channel_id)
            })?;
            let target = ChannelTarget {
                user_id: user_id.clone(),
            };

            let result = process_text(
                &text,
                &user_id,
                modules,
                channel.as_ref(),
                &target,
                &channel_id,
                event_bus,
                debug_output,
                false,
                conversation_config,
                tools_config,
                cache_config,
                memory_config,
                None,
                false,
            )
            .await?;

            // Only call send_message when the text was NOT already streamed.
            if !result.streamed {
                debug_component_text(
                    debug_output,
                    "channel",
                    "send_message",
                    &result.text,
                    Some((&channel_id, &user_id)),
                );
                if let Err(error) = channel.send_message(&target, &result.text).await {
                    log_channel_delivery_failure(channel.as_ref(), error).await;
                }
            }

            Ok(())
        }
        ChannelEvent::InterruptActiveTurn {
            channel_id,
            user_id,
            reason,
        } => {
            let interrupted = voice_turns.interrupt(&channel_id, &user_id).await;
            if interrupted {
                tracing::info!(channel_id, user_id, reason, "active voice turn interrupted");
                if let Some(channel) = channels.get(&channel_id).await {
                    let config = channel.interrupt_config();
                    if config.enabled {
                        let target = ChannelTarget {
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
                    }
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
            let channel = channels.get(&channel_id).await.ok_or_else(|| {
                anyhow::anyhow!("channel '{}' not found for response delivery", channel_id)
            })?;
            let target = ChannelTarget {
                user_id: user_id.clone(),
            };
            let turn = voice_turns
                .begin(
                    channel_id.clone(),
                    user_id.clone(),
                    channel.interrupt_config(),
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
            let transcript = match cancellable_stage(
                Some(&turn),
                "stt",
                modules
                    .stt
                    .transcribe(&audio.data, AudioEncoding::Pcm16kMono),
            )
            .await
            {
                Ok(Some(transcript)) => transcript,
                Ok(None) => {
                    voice_turns.finish(&turn).await;
                    return Ok(());
                }
                Err(error) => {
                    voice_turns.finish(&turn).await;
                    return Err(error);
                }
            };
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

            let result = match process_text(
                &transcript,
                &user_id,
                modules,
                channel.as_ref(),
                &target,
                &channel_id,
                event_bus,
                debug_output,
                channel.capabilities().voice_output && channel.capabilities().audio_streaming,
                conversation_config,
                tools_config,
                cache_config,
                memory_config,
                Some(&turn),
                turn.interrupt_config.discard_interrupted_turn_history,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    voice_turns.finish(&turn).await;
                    return Err(error);
                }
            };

            if result.interrupted {
                voice_turns.finish(&turn).await;
                return Ok(());
            }

            // Text delivery: only if not already streamed
            if !result.streamed {
                debug_component_text(
                    debug_output,
                    "channel",
                    "send_message",
                    &result.text,
                    Some((&channel_id, &user_id)),
                );
                if let Err(error) = channel.send_message(&target, &result.text).await {
                    log_channel_delivery_failure(channel.as_ref(), error).await;
                    voice_turns.finish(&turn).await;
                    return Ok(());
                }
            }

            if channel.capabilities().voice_output && !result.audio_streamed {
                deliver_tts_audio(
                    modules,
                    channel.as_ref(),
                    &target,
                    &channel_id,
                    &user_id,
                    &result.text,
                    debug_output,
                    Some(&turn),
                )
                .await;
            }

            if turn_is_interrupted(Some(&turn)) {
                voice_turns.finish(&turn).await;
                return Ok(());
            }

            if let Some(pending) = result.pending_persistence {
                persist_text_interaction(modules, pending, memory_config).await;
            }

            voice_turns.finish(&turn).await;
            Ok(())
        }
        ChannelEvent::SensorData {
            channel_id,
            sensor_type,
            payload,
        } => {
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

fn channel_event_id(event: &ChannelEvent) -> Option<&str> {
    match event {
        ChannelEvent::TextMessage { channel_id, .. }
        | ChannelEvent::AudioInput { channel_id, .. }
        | ChannelEvent::InterruptActiveTurn { channel_id, .. }
        | ChannelEvent::SensorData { channel_id, .. }
        | ChannelEvent::StatusChange { channel_id, .. } => Some(channel_id),
    }
}

fn stable_user_uuid(user_id: &str) -> Uuid {
    let user_id = user_id.trim();
    if user_id.is_empty() {
        Uuid::nil()
    } else {
        Uuid::parse_str(user_id).unwrap_or_else(|_| {
            let digest = Sha256::digest(user_id.as_bytes());
            let mut bytes = [0_u8; 16];
            bytes.copy_from_slice(&digest[..16]);
            bytes[6] = (bytes[6] & 0x0f) | 0x50;
            bytes[8] = (bytes[8] & 0x3f) | 0x80;
            Uuid::from_bytes(bytes)
        })
    }
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
    conversation_config: &ConversationSection,
    tools_config: &ToolsSection,
    cache_config: &CacheSection,
    memory_config: &MemorySection,
    turn: Option<&VoiceTurn>,
    defer_persistence: bool,
) -> anyhow::Result<TextResult> {
    let uid = stable_user_uuid(user_id);
    let (mut base_system_messages, runtime_context) =
        build_base_system_messages(uid, modules, memory_config, text).await?;
    let memory_context_used = runtime_context_has_user_memory(&runtime_context);

    // 1. Get active session and load history
    let session_id = modules
        .conversation
        .get_or_create_active_session(channel_id, user_id)
        .await?;
    let history = modules
        .conversation
        .get_history(
            session_id,
            conversation_config.max_context_messages.max(1) as u64,
        )
        .await?;
    let mut trace = AgentTrace::new(session_id);
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
                "memory_context_used": memory_context_used,
            }),
        )
        .await;

    // 2. Check exact cache first (never streamed). Only entries with explicit
    // safe-direct metadata can bypass the LLM.
    if cache_config.enabled && cache_config.exact_match_enabled && !memory_context_used {
        if let Some(cached) = modules.cache.get(Some(uid), text).await? {
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
                    persist_text_interaction(modules, pending.clone(), memory_config).await;
                }
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
                return Ok(TextResult {
                    text: response_text,
                    streamed: false,
                    audio_streamed: false,
                    interrupted: false,
                    pending_persistence: defer_persistence.then_some(pending),
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

    if cache_config.enabled
        && cache_config.semantic_match_enabled
        && !memory_context_used
        && let Some((prompt, candidate_count)) =
            semantic_cache_reference_prompt(modules, uid, text, debug_output).await
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
    };
    let decision =
        match cancellable_stage(turn, "router", modules.router.route(&route_input)).await? {
            Some(decision) => decision,
            None => {
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
            let resolution = modules.skills.resolve(&skill_match).await?;
            trace
                .record(
                    modules,
                    "skill_resolved",
                    "skills",
                    Some(resolution.skill_name.clone()),
                    serde_json::json!({
                        "skill": resolution.skill_name,
                        "cacheable": resolution.cacheable,
                        "tools": resolution.tools,
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
            messages.extend(history);
            messages.push(ChatMessage {
                role: "user".into(),
                content: text.to_string(),
                name: Some(user_id.to_string()),
                ..ChatMessage::default()
            });

            let tools =
                select_tools_for_skill(modules, &resolution.tools, text, tools_config, channel_id)
                    .await?;
            let skill_cacheable = resolution.cacheable;
            let outcome = execute_llm_response(
                modules,
                modules.llm.as_ref(),
                messages,
                tools,
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
            )
            .await?;
            (outcome, Some(skill_cacheable), "skill")
        }

        // LLM call — stream when the channel supports it
        RouteDecision::LocalLlm {
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
            let tools = select_all_tools(modules, text, tools_config, channel_id).await?;
            let allow_local_streaming =
                conversation_config.streaming_enabled && escalation_prompt.is_none();
            let local_outcome = execute_llm_response(
                modules,
                modules.local_llm.as_ref(),
                messages,
                tools,
                uid,
                session_id,
                channel,
                target,
                channel_id,
                user_id,
                event_bus,
                "local_llm",
                audio_response,
                allow_local_streaming,
                tools_config,
                debug_output,
                &mut trace,
                turn,
            )
            .await?;

            if let Some(reason) = escalation_reason(&local_outcome.text)
                && let Some(remote_prompt) = escalation_prompt
            {
                tracing::info!(
                    channel_id,
                    user_id,
                    reason = %reason,
                    "local LLM escalated turn to remote LLM"
                );
                debug_component_text(
                    debug_output,
                    "llm",
                    "escalate",
                    reason,
                    Some((channel_id, user_id)),
                );
                let mut remote_messages = build_llm_messages(
                    &base_system_messages,
                    &history,
                    Some(remote_prompt),
                    text,
                    user_id,
                );
                remote_messages.push(ChatMessage::new(
                    "system",
                    format!(
                        "The local model escalated this request instead of answering. Escalation reason: {reason}. Answer the original user request directly."
                    ),
                ));
                let remote_tools =
                    select_all_tools(modules, text, tools_config, channel_id).await?;
                let remote_outcome = execute_llm_response(
                    modules,
                    modules.remote_llm.as_ref(),
                    remote_messages,
                    remote_tools,
                    uid,
                    session_id,
                    channel,
                    target,
                    channel_id,
                    user_id,
                    event_bus,
                    "remote_llm_escalated",
                    audio_response,
                    conversation_config.streaming_enabled,
                    tools_config,
                    debug_output,
                    &mut trace,
                    turn,
                )
                .await?;
                (remote_outcome, None, "remote_llm_escalated")
            } else {
                (local_outcome, None, "local_llm")
            }
        }
        RouteDecision::RemoteLlm { system_prompt } => {
            let messages = build_llm_messages(
                &base_system_messages,
                &history,
                system_prompt,
                text,
                user_id,
            );
            let tools = select_all_tools(modules, text, tools_config, channel_id).await?;
            let outcome = execute_llm_response(
                modules,
                modules.remote_llm.as_ref(),
                messages,
                tools,
                uid,
                session_id,
                channel,
                target,
                channel_id,
                user_id,
                event_bus,
                "remote_llm",
                audio_response,
                conversation_config.streaming_enabled,
                tools_config,
                debug_output,
                &mut trace,
                turn,
            )
            .await?;
            (outcome, None, "remote_llm")
        }
    };

    if turn_is_interrupted(turn) {
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
    let cache_write = cache_write_for_response(cache_config, text, &response, cache_context);
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
        persist_text_interaction(modules, pending_persistence.clone(), memory_config).await;
    }
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

    Ok(TextResult {
        text: response,
        streamed,
        audio_streamed,
        interrupted: false,
        pending_persistence: defer_persistence.then_some(pending_persistence),
    })
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
    messages.extend_from_slice(history);
    messages.push(ChatMessage {
        role: "user".into(),
        content: text.to_string(),
        name: Some(user_id.to_string()),
        ..ChatMessage::default()
    });
    messages
}

#[allow(clippy::too_many_arguments)]
async fn execute_llm_response(
    modules: &ModuleRegistry,
    llm: &dyn LlmAdapter,
    messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
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
) -> anyhow::Result<LlmExecutionOutcome> {
    if !tools.is_empty() {
        debug_component_tools(
            debug_output,
            "tools",
            "available",
            &tools,
            Some((channel_id, user_id)),
        );
        let available_tools = tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        trace
            .record(
                modules,
                "llm_call",
                "llm",
                Some(route.to_string()),
                serde_json::json!({
                    "route": route,
                    "mode": "tool_loop",
                    "available_tools": available_tools,
                    "messages": messages.len(),
                }),
            )
            .await;
        let outcome = run_llm_tool_loop(
            modules,
            llm,
            messages,
            tools,
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
        let (full, audio_streamed) = if audio_response {
            stream_llm_response_with_sentence_tts(
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
        };
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
        "local model requested escalation"
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
}

pub fn cache_write_for_response(
    cache_config: &CacheSection,
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
        || matches!(context.route, "router_cache" | "remote_llm_escalated")
    {
        return None;
    }
    if matches!(context.skill_cacheable, Some(false)) {
        return None;
    }
    if query_is_dynamic(query_text)
        || query_is_memory_related(query_text)
        || query_is_device_or_action_related(query_text)
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
            "safe_direct_return": true,
            "memory_independent": true,
            "tool_free": true,
            "dynamic_free": true,
            "route": context.route,
            "language": guess_text_language(query_text),
            "created_by": "pipeline",
        }),
        ttl_seconds,
        store_embedding: cache_config.semantic_match_enabled,
    })
}

fn runtime_context_has_user_memory(context: &RuntimePromptContext) -> bool {
    !context.core_memories.is_empty() || !context.user_preferences.is_empty()
}

async fn semantic_cache_reference_prompt(
    modules: &ModuleRegistry,
    user_id: Uuid,
    query_text: &str,
    debug_output: bool,
) -> Option<(String, usize)> {
    if modules.embedding.name() == "noop_embedding" {
        return None;
    }

    let embeddings = match modules.embedding.embed(&[query_text.to_string()]).await {
        Ok(embeddings) => embeddings,
        Err(error) => {
            tracing::debug!(error = ?error, "semantic cache embedding lookup failed");
            return None;
        }
    };
    let embedding = embeddings.first()?;
    let candidates = match modules
        .cache
        .get_semantic_candidates(Some(user_id), embedding, 3)
        .await
    {
        Ok(candidates) => candidates
            .into_iter()
            .filter(semantic_candidate_is_safe_reference)
            .collect::<Vec<_>>(),
        Err(error) => {
            tracing::debug!(error = ?error, "semantic cache candidate lookup failed");
            Vec::new()
        }
    };
    if candidates.is_empty() {
        return None;
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

fn semantic_candidate_is_safe_reference(candidate: &SemanticCacheCandidate) -> bool {
    cached_response_is_safe_direct(&CachedResponse {
        response_text: candidate.response_text.clone(),
        metadata: candidate.metadata.clone(),
    })
}

fn preview_for_prompt(text: &str, max_chars: usize) -> String {
    let mut preview = text.trim().chars().take(max_chars).collect::<String>();
    if text.trim().chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

fn query_is_dynamic(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "today",
            "tomorrow",
            "now",
            "latest",
            "current",
            "weather",
            "news",
            "time",
            "date",
            "search",
            "fetch",
            "lookup",
            "http://",
            "https://",
            "今天",
            "明天",
            "现在",
            "最新",
            "当前",
            "天气",
            "新闻",
            "时间",
            "几点",
            "日期",
            "搜索",
            "查询",
            "查一下",
            "检索",
        ],
    )
}

fn query_is_memory_related(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "remember",
            "memory",
            "memorize",
            "preference",
            "favorite",
            "what do i",
            "my ",
            "mine",
            "记住",
            "记得",
            "记忆",
            "偏好",
            "我的",
            "我喜欢",
            "我最喜欢",
            "之前",
        ],
    )
}

fn query_is_device_or_action_related(text: &str) -> bool {
    let lower = text.to_lowercase();
    contains_any(
        &lower,
        &[
            "turn on",
            "turn off",
            "set ",
            "adjust",
            "volume",
            "device",
            "light",
            "speaker",
            "microphone",
            "schedule",
            "remind",
            "notify",
            "打开",
            "关闭",
            "设置",
            "调整",
            "调大",
            "调小",
            "音量",
            "设备",
            "灯",
            "麦克风",
            "提醒",
            "通知",
        ],
    )
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
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
) {
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

    if memory_config.enabled
        && memory_config.conversation_ingest_enabled
        && !pending.user_id.is_nil()
    {
        let ingest = MemoryConversationIngest {
            run_id: Some(pending.session_id.to_string()),
            messages: vec![
                MemoryConversationMessage {
                    role: "user".to_string(),
                    content: pending.query_text,
                    name: Some(pending.user_id_text),
                },
                MemoryConversationMessage {
                    role: "assistant".to_string(),
                    content: pending.response_text,
                    name: None,
                },
            ],
            metadata: serde_json::json!({
                "source": "pipeline",
                "channel_id": pending.channel_id,
                "conversation_id": pending.session_id,
            }),
        };

        if let Err(error) = modules
            .memory
            .ingest_conversation(pending.user_id, ingest)
            .await
        {
            tracing::warn!(
                user_id = %pending.user_id,
                session_id = %pending.session_id,
                error = ?error,
                "failed to ingest conversation into memory backend"
            );
        }
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

    match modules.persona.render_system_prompt(user_id).await {
        Ok(prompt) if !prompt.trim().is_empty() => {
            messages.push(system_message(prompt));
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(user_id = %user_id, error = ?error, "failed to render persona prompt");
        }
    }

    let runtime_context =
        build_runtime_prompt_context(user_id, modules, memory_config, user_text).await;
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

    let core_memories = retrieve_runtime_memories(user_id, modules, memory_config, user_text).await;

    RuntimePromptContext {
        current_time: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        core_memories,
        available_skills,
        ..RuntimePromptContext::default()
    }
}

async fn retrieve_runtime_memories(
    user_id: Uuid,
    modules: &ModuleRegistry,
    memory_config: &MemorySection,
    user_text: &str,
) -> Vec<String> {
    if !memory_config.enabled || user_id.is_nil() || user_text.trim().is_empty() {
        return Vec::new();
    }

    match modules
        .memory
        .retrieve(
            user_id,
            user_text,
            RetrievalOpts {
                max_results: Some(memory_config.max_retrieval_results.max(1).min(12)),
                min_similarity: Some(memory_config.similarity_threshold.clamp(0.0, 1.0)),
                layers: None,
            },
        )
        .await
    {
        Ok(nodes) => nodes
            .into_iter()
            .filter(|node| !node.content.trim().is_empty())
            .map(|node| node.content)
            .collect(),
        Err(error) => {
            tracing::warn!(user_id = %user_id, error = ?error, "failed to retrieve runtime memories");
            Vec::new()
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

    if !context.core_memories.is_empty() {
        let lines = context
            .core_memories
            .iter()
            .filter(|memory| !memory.trim().is_empty())
            .map(|memory| format!("- {}", memory.trim()))
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            sections.push(format!("## User Memory\n{}", lines.join("\n")));
        }
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

fn system_message(content: String) -> ChatMessage {
    ChatMessage {
        role: "system".into(),
        content,
        name: None,
        ..ChatMessage::default()
    }
}
