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

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::channels::ChannelRegistry;
use crate::channels::adapter::{ChannelAdapter, ChannelEvent, ChannelTarget};
use crate::config::ToolsSection;
use crate::event::{EventBus, ServerEvent};
use crate::module::ModuleRegistry;
use crate::providers::adapter::{
    AudioEncoding, CachedResponse, ChatMessage, RouteDecision, RouterInput,
};
use crate::providers::sensor::SensorRegistry;

mod debug;
mod streaming;
mod tools;
mod turn;
use debug::*;
use streaming::*;
use tools::*;
use turn::*;

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
struct PendingTextPersistence {
    session_id: Uuid,
    user_id: Uuid,
    user_id_text: String,
    query_text: String,
    response_text: String,
}

#[derive(Debug, Clone, Default)]
struct RuntimePromptContext {
    core_memories: Vec<String>,
    user_preferences: Vec<(String, String)>,
    current_time: String,
    user_presence: String,
    device_status_summary: String,
    available_skills: Vec<String>,
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
    tools_config: ToolsSection,
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
                            let tools_config = tools_config.clone();
                            let voice_turns = voice_turns.clone();
                            // Process each event concurrently
                            tokio::spawn(async move {
                                if let Err(e) = process_event(event, &modules, &channels, &sensors, &bus, &voice_turns, debug_output, &tools_config).await {
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
    tools_config: &ToolsSection,
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
                tools_config,
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
                tools_config,
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
                persist_text_interaction(modules, pending).await;
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
                sensor_id,
                value: payload,
                timestamp: chrono::Utc::now(),
                quality: 1.0,
            };
            sensors.ingest(reading).await;
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
    tools_config: &ToolsSection,
    turn: Option<&VoiceTurn>,
    defer_persistence: bool,
) -> anyhow::Result<TextResult> {
    let uid = Uuid::parse_str(user_id).unwrap_or_else(|_| Uuid::nil());
    let base_system_messages = build_base_system_messages(uid, modules).await?;

    // 1. Get active session and load history
    let session_id = modules
        .conversation
        .get_or_create_active_session(channel_id, user_id)
        .await?;
    let history = modules.conversation.get_history(session_id, 20).await?;

    // 2. Check cache first (never streamed)
    if let Some(CachedResponse { response_text, .. }) = modules.cache.get(Some(uid), text).await? {
        tracing::debug!(query = text, "pipeline cache hit");
        debug_component_text(
            debug_output,
            "cache",
            "hit",
            &response_text,
            Some((channel_id, user_id)),
        );
        return Ok(TextResult {
            text: response_text,
            streamed: false,
            audio_streamed: false,
            interrupted: false,
            pending_persistence: None,
        });
    }
    debug_component(debug_output, "cache", "miss", Some((channel_id, user_id)));

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

    let (response, streamed, audio_streamed) = match decision {
        // Cache hit from router — never streamed
        RouteDecision::CacheHit { response } => {
            debug_component_text(
                debug_output,
                "router_cache",
                "result",
                &response,
                Some((channel_id, user_id)),
            );
            (response, false, false)
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

            let tools = select_tools_for_skill(modules, &resolution.tools, tools_config).await?;
            if !tools.is_empty() {
                debug_component_tools(
                    debug_output,
                    "tools",
                    "available",
                    &tools,
                    Some((channel_id, user_id)),
                );
                let full = run_llm_tool_loop(
                    modules,
                    messages,
                    tools,
                    uid,
                    Some(session_id),
                    channel_id,
                    user_id,
                    "skill",
                    tools_config,
                    debug_output,
                    turn,
                )
                .await?;
                debug_component_text(
                    debug_output,
                    "llm",
                    "tool_loop_result",
                    &full,
                    Some((channel_id, user_id)),
                );
                (full, false, false)
            } else if channel.capabilities().streaming {
                debug_component_llm_call(
                    debug_output,
                    "llm",
                    "stream_call",
                    "skill",
                    &messages,
                    Some((channel_id, user_id)),
                );
                let (full, audio_streamed) = if audio_response {
                    stream_llm_response_with_sentence_tts(
                        modules,
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
                (full, true, audio_streamed)
            } else {
                debug_component_llm_call(
                    debug_output,
                    "llm",
                    "call",
                    "skill",
                    &messages,
                    Some((channel_id, user_id)),
                );
                let full = match cancellable_stage(turn, "llm", modules.llm.chat(&messages)).await?
                {
                    Some(full) => full,
                    None => String::new(),
                };
                if turn_is_interrupted(turn) {
                    (full, false, false)
                } else {
                    debug_component_text(
                        debug_output,
                        "llm",
                        "result",
                        &full,
                        Some((channel_id, user_id)),
                    );
                    (full, false, false)
                }
            }
        }

        // LLM call — stream when the channel supports it
        RouteDecision::LocalLlm { system_prompt } | RouteDecision::RemoteLlm { system_prompt } => {
            let mut messages = base_system_messages.clone();
            if let Some(sp) = system_prompt {
                messages.push(ChatMessage {
                    role: "system".into(),
                    content: sp,
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

            let tools = select_all_tools(modules, tools_config).await?;
            if !tools.is_empty() {
                debug_component_tools(
                    debug_output,
                    "tools",
                    "available",
                    &tools,
                    Some((channel_id, user_id)),
                );
                let full = run_llm_tool_loop(
                    modules,
                    messages,
                    tools,
                    uid,
                    Some(session_id),
                    channel_id,
                    user_id,
                    "llm",
                    tools_config,
                    debug_output,
                    turn,
                )
                .await?;
                debug_component_text(
                    debug_output,
                    "llm",
                    "tool_loop_result",
                    &full,
                    Some((channel_id, user_id)),
                );
                (full, false, false)
            } else if channel.capabilities().streaming {
                debug_component_llm_call(
                    debug_output,
                    "llm",
                    "stream_call",
                    "llm",
                    &messages,
                    Some((channel_id, user_id)),
                );
                let (full, audio_streamed) = if audio_response {
                    stream_llm_response_with_sentence_tts(
                        modules,
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
                (full, true, audio_streamed)
            } else {
                debug_component_llm_call(
                    debug_output,
                    "llm",
                    "call",
                    "llm",
                    &messages,
                    Some((channel_id, user_id)),
                );
                let full = match cancellable_stage(turn, "llm", modules.llm.chat(&messages)).await?
                {
                    Some(full) => full,
                    None => String::new(),
                };
                if turn_is_interrupted(turn) {
                    (full, false, false)
                } else {
                    debug_component_text(
                        debug_output,
                        "llm",
                        "result",
                        &full,
                        Some((channel_id, user_id)),
                    );
                    (full, false, false)
                }
            }
        }
    };

    if turn_is_interrupted(turn) {
        return Ok(TextResult {
            text: response,
            streamed,
            audio_streamed,
            interrupted: true,
            pending_persistence: None,
        });
    }

    let pending_persistence = PendingTextPersistence {
        session_id,
        user_id: uid,
        user_id_text: user_id.to_string(),
        query_text: text.to_string(),
        response_text: response.clone(),
    };

    if !defer_persistence {
        persist_text_interaction(modules, pending_persistence.clone()).await;
    }

    Ok(TextResult {
        text: response,
        streamed,
        audio_streamed,
        interrupted: false,
        pending_persistence: defer_persistence.then_some(pending_persistence),
    })
}

async fn persist_text_interaction(modules: &ModuleRegistry, pending: PendingTextPersistence) {
    let _ = modules
        .conversation
        .append_message(
            pending.session_id,
            ChatMessage {
                role: "user".into(),
                content: pending.query_text.clone(),
                name: Some(pending.user_id_text),
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

    let _ = modules
        .cache
        .put(crate::providers::adapter::CacheEntry {
            user_id: Some(pending.user_id),
            scope: "pipeline".into(),
            query_text: pending.query_text,
            response_text: pending.response_text,
            metadata: serde_json::Value::Null,
            ttl_seconds: 3600,
        })
        .await;
}

async fn build_base_system_messages(
    user_id: Uuid,
    modules: &ModuleRegistry,
) -> anyhow::Result<Vec<ChatMessage>> {
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

    let runtime_context = build_runtime_prompt_context(modules).await;
    if let Some(prompt) = render_runtime_context_system_prompt(&runtime_context) {
        messages.push(system_message(prompt));
    }

    Ok(messages)
}

async fn build_runtime_prompt_context(modules: &ModuleRegistry) -> RuntimePromptContext {
    let available_skills = modules
        .skills
        .list_skills()
        .await
        .map(|skills| skills.into_iter().map(|skill| skill.display_name).collect())
        .unwrap_or_default();

    RuntimePromptContext {
        current_time: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        available_skills,
        ..RuntimePromptContext::default()
    }
}

fn render_runtime_context_system_prompt(context: &RuntimePromptContext) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::streaming::{pcm16_silence_payload, tts_bytes_to_pcm_payload};
    use super::tools::parse_text_tool_calls;
    use super::{RuntimePromptContext, render_runtime_context_system_prompt};
    use crate::channels::adapter::AudioFormat;

    #[test]
    fn parses_text_tool_call_envelope() {
        let calls = parse_text_tool_calls(
            r#"{"schema":"alia.tool_calls.v1","tool_calls":[{"name":"get_current_time","arguments":{"format":"human"}}]}"#,
        )
        .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_name, "get_current_time");
        assert_eq!(calls[0].arguments["format"], "human");
    }

    #[test]
    fn parses_fenced_text_tool_call_envelope() {
        let calls = parse_text_tool_calls(
            "```json\n{\"schema\":\"alia.tool_calls.v1\",\"tool_calls\":[{\"tool_name\":\"sensor_list\",\"arguments\":\"{}\"}]}\n```",
        )
        .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_name, "sensor_list");
    }

    #[test]
    fn ignores_plain_assistant_text() {
        let calls = parse_text_tool_calls("Sure, I can help with that.").unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn ignores_tool_call_without_schema() {
        let calls =
            parse_text_tool_calls(r#"{"tool_calls":[{"name":"get_current_time","arguments":{}}]}"#)
                .unwrap();

        assert!(calls.is_empty());
    }

    #[test]
    fn ignores_embedded_tool_call_examples() {
        let calls = parse_text_tool_calls(
            r#"Tool use rules:
- If native tool calling is unavailable and a tool is needed, reply with exactly one JSON object and no prose:
  {"tool_calls":[{"name":"tool_name","arguments":{}}]}

Available tools:
[
  {"name":"get_current_time","parameters":{"type":"object"}}
]"#,
        )
        .unwrap();

        assert!(calls.is_empty());
    }

    #[test]
    fn ignores_json_with_trailing_text() {
        let calls = parse_text_tool_calls(
            r#"{"tool_calls":[{"name":"get_current_time","arguments":{}}]}
This is an example, not a call."#,
        )
        .unwrap();

        assert!(calls.is_empty());
    }

    #[test]
    fn ignores_envelope_with_user_facing_content() {
        let calls = parse_text_tool_calls(
            r#"{"schema":"alia.tool_calls.v1","tool_calls":[{"name":"get_current_time","arguments":{}}],"content":"I will call a tool."}"#,
        )
        .unwrap();

        assert!(calls.is_empty());
    }

    #[test]
    fn ignores_text_tool_call_with_non_object_arguments() {
        let calls = parse_text_tool_calls(
            r#"{"schema":"alia.tool_calls.v1","tool_calls":[{"name":"get_current_time","arguments":"not json"}]}"#,
        )
        .unwrap();

        assert!(calls.is_empty());
    }

    #[test]
    fn renders_runtime_context_prompt_outside_persona() {
        let prompt = render_runtime_context_system_prompt(&RuntimePromptContext {
            core_memories: vec!["The user likes coffee.".to_string()],
            user_preferences: vec![("Temperature".to_string(), "26 C".to_string())],
            current_time: "2026-05-16 16:30".to_string(),
            user_presence: "At home".to_string(),
            device_status_summary: "Living room speaker is online".to_string(),
            available_skills: vec!["Climate control".to_string()],
        })
        .unwrap();

        assert!(prompt.contains("## Runtime Context"));
        assert!(prompt.contains("The user likes coffee."));
        assert!(prompt.contains("## Skill Routing Context"));
        assert!(prompt.contains("They are not functions to call directly"));
    }

    #[test]
    fn decodes_tts_wav_to_pcm_payload() {
        let pcm = [1i16, -2i16, 3i16, -4i16]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let wav = test_wav(&pcm, 16_000, 2);

        let payload = tts_bytes_to_pcm_payload(wav).unwrap();

        assert_eq!(payload.format, AudioFormat::Pcm16);
        assert_eq!(payload.sample_rate, 16_000);
        assert_eq!(payload.channels, 2);
        assert_eq!(payload.data, pcm);
    }

    #[test]
    fn builds_pcm_silence_gap() {
        let payload = pcm16_silence_payload(16_000, 1, 80);
        assert_eq!(payload.format, AudioFormat::Pcm16);
        assert_eq!(payload.data.len(), 16_000 * 80 / 1_000 * 2);
        assert!(payload.data.iter().all(|byte| *byte == 0));
    }

    fn test_wav(pcm: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
        let mut wav = Vec::new();
        let data_len = pcm.len() as u32;
        let byte_rate = sample_rate * u32::from(channels) * 2;
        let block_align = channels * 2;
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
}
