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

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::channels::ChannelRegistry;
use crate::channels::adapter::{AudioPayload, ChannelAdapter, ChannelEvent, ChannelTarget};
use crate::event::{EventBus, ServerEvent};
use crate::module::ModuleRegistry;
use crate::providers::adapter::{
    AudioEncoding, CachedResponse, ChatMessage, RouteDecision, RouterInput,
};
use crate::providers::sensor::SensorRegistry;

/// Result of `process_text` — carries the full response text and whether it
/// was already delivered to the channel via streaming.
struct TextResult {
    text: String,
    /// `true` when tokens were already delivered via `begin_stream / send_stream_chunk
    /// / end_stream`.  Callers must NOT call `send_message` again.
    streamed: bool,
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
) {
    let mut rx = event_bus.subscribe();
    let bus = event_bus.clone();

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
                            // Process each event concurrently
                            tokio::spawn(async move {
                                if let Err(e) = process_event(event, &modules, &channels, &sensors, &bus, debug_output).await {
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
    debug_output: bool,
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
        ChannelEvent::AudioInput {
            channel_id,
            user_id,
            audio,
        } => {
            // STT → Router → LLM → TTS
            debug_component_audio(
                debug_output,
                "stt",
                "call",
                audio.data.len(),
                Some((&channel_id, &user_id)),
            );
            let transcript = modules
                .stt
                .transcribe(&audio.data, AudioEncoding::Pcm16kMono)
                .await?;
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
                return Ok(());
            }

            let channel = channels.get(&channel_id).await.ok_or_else(|| {
                anyhow::anyhow!("channel '{}' not found for response delivery", channel_id)
            })?;
            let target = ChannelTarget {
                user_id: user_id.clone(),
            };

            let result = process_text(
                &transcript,
                &user_id,
                modules,
                channel.as_ref(),
                &target,
                &channel_id,
                event_bus,
                debug_output,
            )
            .await?;

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
                    return Ok(());
                }
            }

            // Audio delivery: always send TTS regardless of streaming
            debug_component_text(
                debug_output,
                "tts",
                "call",
                &result.text,
                Some((&channel_id, &user_id)),
            );
            let audio_bytes = modules.tts.synthesize(&result.text).await?;
            debug_component_audio(
                debug_output,
                "tts",
                "result",
                audio_bytes.len(),
                Some((&channel_id, &user_id)),
            );
            let audio_out = AudioPayload {
                data: audio_bytes,
                sample_rate: 24000,
                channels: 1,
                format: crate::channels::adapter::AudioFormat::Wav,
            };

            if channel.capabilities().voice_output {
                debug_component_audio(
                    debug_output,
                    "channel",
                    "send_audio",
                    audio_out.data.len(),
                    Some((&channel_id, &user_id)),
                );
                if let Err(error) = channel.send_audio(&target, audio_out).await {
                    log_channel_delivery_failure(channel.as_ref(), error).await;
                }
            }

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
) -> anyhow::Result<TextResult> {
    let uid = Uuid::parse_str(user_id).unwrap_or_else(|_| Uuid::nil());

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
    let decision = modules.router.route(&route_input).await?;
    debug_component_route(
        debug_output,
        "router",
        "result",
        route_decision_name(&decision),
        Some((channel_id, user_id)),
    );

    let (response, streamed) = match decision {
        // Cache hit from router — never streamed
        RouteDecision::CacheHit { response } => {
            debug_component_text(
                debug_output,
                "router_cache",
                "result",
                &response,
                Some((channel_id, user_id)),
            );
            (response, false)
        }

        // Skill execution — resolve skill, then send through LLM with tools.
        // Skills extend what the LLM knows how to do well; the LLM decides
        // which tools to call (if any) based on the skill's system prompt.
        RouteDecision::Skill { skill_name, params } => {
            let skill_match = crate::providers::adapter::SkillMatch {
                name: skill_name,
                confidence: 1.0,
                parameters: params.clone(),
            };
            let resolution = modules.skills.resolve(&skill_match).await?;

            // Build messages with the skill's domain-specific system prompt
            let mut messages = Vec::new();
            if let Some(sp) = &resolution.system_prompt {
                messages.push(ChatMessage {
                    role: "system".into(),
                    content: sp.clone(),
                    name: None,
                });
            }
            messages.extend(history);
            messages.push(ChatMessage {
                role: "user".into(),
                content: text.to_string(),
                name: Some(user_id.to_string()),
            });

            // TODO: When the LLM adapter supports tool/function calling, filter
            // available tools to `resolution.tools` and pass them alongside the
            // messages. For now, use a standard LLM call.
            if channel.capabilities().streaming {
                debug_component_llm_call(
                    debug_output,
                    "llm",
                    "stream_call",
                    "skill",
                    &messages,
                    Some((channel_id, user_id)),
                );
                let full = stream_llm_response(
                    modules,
                    channel,
                    target,
                    channel_id,
                    user_id,
                    event_bus,
                    &messages,
                    debug_output,
                )
                .await?;
                debug_component_text(
                    debug_output,
                    "llm",
                    "stream_result",
                    &full,
                    Some((channel_id, user_id)),
                );
                (full, true)
            } else {
                debug_component_llm_call(
                    debug_output,
                    "llm",
                    "call",
                    "skill",
                    &messages,
                    Some((channel_id, user_id)),
                );
                let full = modules.llm.chat(&messages).await?;
                debug_component_text(
                    debug_output,
                    "llm",
                    "result",
                    &full,
                    Some((channel_id, user_id)),
                );
                (full, false)
            }
        }

        // LLM call — stream when the channel supports it
        RouteDecision::LocalLlm { system_prompt } | RouteDecision::RemoteLlm { system_prompt } => {
            let mut messages = Vec::new();
            if let Some(sp) = system_prompt {
                messages.push(ChatMessage {
                    role: "system".into(),
                    content: sp,
                    name: None,
                });
            }
            messages.extend(history);
            messages.push(ChatMessage {
                role: "user".into(),
                content: text.to_string(),
                name: Some(user_id.to_string()),
            });

            if channel.capabilities().streaming {
                debug_component_llm_call(
                    debug_output,
                    "llm",
                    "stream_call",
                    "llm",
                    &messages,
                    Some((channel_id, user_id)),
                );
                let full = stream_llm_response(
                    modules,
                    channel,
                    target,
                    channel_id,
                    user_id,
                    event_bus,
                    &messages,
                    debug_output,
                )
                .await?;
                debug_component_text(
                    debug_output,
                    "llm",
                    "stream_result",
                    &full,
                    Some((channel_id, user_id)),
                );
                (full, true)
            } else {
                debug_component_llm_call(
                    debug_output,
                    "llm",
                    "call",
                    "llm",
                    &messages,
                    Some((channel_id, user_id)),
                );
                let full = modules.llm.chat(&messages).await?;
                debug_component_text(
                    debug_output,
                    "llm",
                    "result",
                    &full,
                    Some((channel_id, user_id)),
                );
                (full, false)
            }
        }
    };

    // 4. Append to session history
    let _ = modules
        .conversation
        .append_message(
            session_id,
            ChatMessage {
                role: "user".into(),
                content: text.to_string(),
                name: Some(user_id.to_string()),
            },
        )
        .await;

    let _ = modules
        .conversation
        .append_message(
            session_id,
            ChatMessage {
                role: "assistant".into(),
                content: response.clone(),
                name: None,
            },
        )
        .await;

    // 5. Store in cache for next time
    let _ = modules
        .cache
        .put(crate::providers::adapter::CacheEntry {
            user_id: Some(uid),
            scope: "pipeline".into(),
            query_text: text.to_string(),
            response_text: response.clone(),
            metadata: serde_json::Value::Null,
            ttl_seconds: 3600,
        })
        .await;

    Ok(TextResult {
        text: response,
        streamed,
    })
}

/// Stream an LLM response token-by-token to the channel.
///
/// Calls `begin_stream`, then forwards each token via `send_stream_chunk` and
/// publishes `StreamChunk` events on the EventBus, then calls `end_stream` and
/// publishes `StreamEnd`.  Returns the full concatenated response.
async fn stream_llm_response(
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    event_bus: &EventBus,
    messages: &[ChatMessage],
    debug_output: bool,
) -> anyhow::Result<String> {
    let mut token_stream = modules.llm.chat_stream(messages).await?;

    debug_component(
        debug_output,
        "channel",
        "begin_stream",
        Some((channel_id, user_id)),
    );
    if let Err(error) = channel.begin_stream(target).await {
        tracing::warn!(channel_id, error = %error, "stream begin delivery failed");
        tracing::debug!(channel_id, error = ?error, "stream begin delivery failure details");
    }

    let mut full_text = String::new();

    while let Some(chunk_result) = token_stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                full_text.push_str(&chunk);
                debug_component_stream_chunk(debug_output, &chunk, Some((channel_id, user_id)));

                // Deliver to the channel
                if let Err(e) = channel.send_stream_chunk(target, &chunk).await {
                    tracing::warn!(error = %e, "stream chunk delivery failed");
                    tracing::debug!(error = ?e, "stream chunk delivery failure details");
                }

                // Publish on event bus for WS subscribers
                event_bus.publish(ServerEvent::StreamChunk {
                    channel_id: channel_id.to_string(),
                    user_id: user_id.to_string(),
                    chunk,
                });
            }
            Err(e) => {
                tracing::error!(error = %e, "LLM stream error");
                tracing::debug!(error = ?e, "LLM stream error details");
                break;
            }
        }
    }

    if let Err(error) = channel.end_stream(target, &full_text).await {
        tracing::warn!(channel_id, error = %error, "stream end delivery failed");
        tracing::debug!(channel_id, error = ?error, "stream end delivery failure details");
    }
    debug_component_text(
        debug_output,
        "channel",
        "end_stream",
        &full_text,
        Some((channel_id, user_id)),
    );

    event_bus.publish(ServerEvent::StreamEnd {
        channel_id: channel_id.to_string(),
        user_id: user_id.to_string(),
    });

    Ok(full_text)
}

async fn log_channel_delivery_failure(channel: &dyn ChannelAdapter, error: anyhow::Error) {
    let channel_type = channel.channel_type();
    let channel_id = channel.channel_id().to_string();

    tracing::warn!(
        channel_type,
        channel_id = %channel_id,
        error = %error,
        "channel delivery failed; keeping channel enabled"
    );
    tracing::debug!(
        channel_type,
        channel_id = %channel_id,
        error = ?error,
        "channel delivery failure details"
    );
}

fn route_decision_name(decision: &RouteDecision) -> &'static str {
    match decision {
        RouteDecision::CacheHit { .. } => "cache_hit",
        RouteDecision::Skill { .. } => "skill",
        RouteDecision::LocalLlm { .. } => "local_llm",
        RouteDecision::RemoteLlm { .. } => "remote_llm",
    }
}

fn debug_component(
    enabled: bool,
    component: &'static str,
    event: &'static str,
    channel: Option<(&str, &str)>,
) {
    if !enabled {
        return;
    }

    match channel {
        Some((channel_id, user_id)) => tracing::info!(
            target: "alia_server::debug_output",
            component,
            event,
            channel_id,
            user_id,
            "component event"
        ),
        None => tracing::info!(
            target: "alia_server::debug_output",
            component,
            event,
            "component event"
        ),
    }
}

fn debug_component_text(
    enabled: bool,
    component: &'static str,
    event: &'static str,
    content: &str,
    channel: Option<(&str, &str)>,
) {
    if !enabled {
        tracing::debug!(
            target: "alia_server::debug_output",
            component,
            event,
            content = %preview_text(content, 120),
            "component text"
        );
        return;
    }

    match channel {
        Some((channel_id, user_id)) => tracing::info!(
            target: "alia_server::debug_output",
            component,
            event,
            channel_id,
            user_id,
            content = %preview_text(content, 240),
            "component text"
        ),
        None => tracing::info!(
            target: "alia_server::debug_output",
            component,
            event,
            content = %preview_text(content, 240),
            "component text"
        ),
    }
}

fn debug_component_audio(
    enabled: bool,
    component: &'static str,
    event: &'static str,
    bytes: usize,
    channel: Option<(&str, &str)>,
) {
    if !enabled {
        tracing::debug!(
            target: "alia_server::debug_output",
            component,
            event,
            bytes,
            "component audio"
        );
        return;
    }

    match channel {
        Some((channel_id, user_id)) => tracing::info!(
            target: "alia_server::debug_output",
            component,
            event,
            channel_id,
            user_id,
            bytes,
            "component audio"
        ),
        None => tracing::info!(
            target: "alia_server::debug_output",
            component,
            event,
            bytes,
            "component audio"
        ),
    }
}

fn debug_component_route(
    enabled: bool,
    component: &'static str,
    event: &'static str,
    decision: &'static str,
    channel: Option<(&str, &str)>,
) {
    if !enabled {
        tracing::debug!(
            target: "alia_server::debug_output",
            component,
            event,
            decision,
            "component route"
        );
        return;
    }

    match channel {
        Some((channel_id, user_id)) => tracing::info!(
            target: "alia_server::debug_output",
            component,
            event,
            channel_id,
            user_id,
            decision,
            "component route"
        ),
        None => tracing::info!(
            target: "alia_server::debug_output",
            component,
            event,
            decision,
            "component route"
        ),
    }
}

fn debug_component_llm_call(
    enabled: bool,
    component: &'static str,
    event: &'static str,
    route: &'static str,
    messages: &[ChatMessage],
    channel: Option<(&str, &str)>,
) {
    if !enabled {
        tracing::debug!(
            target: "alia_server::debug_output",
            component,
            event,
            route,
            messages = messages.len(),
            prompt = %last_user_message_preview(messages),
            "component LLM call"
        );
        return;
    }

    match channel {
        Some((channel_id, user_id)) => tracing::info!(
            target: "alia_server::debug_output",
            component,
            event,
            route,
            channel_id,
            user_id,
            messages = messages.len(),
            prompt = %last_user_message_preview(messages),
            "component LLM call"
        ),
        None => tracing::info!(
            target: "alia_server::debug_output",
            component,
            event,
            route,
            messages = messages.len(),
            prompt = %last_user_message_preview(messages),
            "component LLM call"
        ),
    }
}

fn debug_component_stream_chunk(enabled: bool, chunk: &str, channel: Option<(&str, &str)>) {
    if !enabled {
        tracing::debug!(
            target: "alia_server::debug_output",
            component = "llm",
            event = "stream_chunk",
            chars = chunk.chars().count(),
            content = %preview_text(chunk, 80),
            "component stream chunk"
        );
        return;
    }

    match channel {
        Some((channel_id, user_id)) => tracing::debug!(
            target: "alia_server::debug_output",
            component = "llm",
            event = "stream_chunk",
            channel_id,
            user_id,
            chars = chunk.chars().count(),
            content = %preview_text(chunk, 80),
            "component stream chunk"
        ),
        None => tracing::debug!(
            target: "alia_server::debug_output",
            component = "llm",
            event = "stream_chunk",
            chars = chunk.chars().count(),
            content = %preview_text(chunk, 80),
            "component stream chunk"
        ),
    }
}

fn last_user_message_preview(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| preview_text(&message.content, 180))
        .unwrap_or_else(|| "<no user message>".to_string())
}

fn preview_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}
