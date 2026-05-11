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
                            let modules = modules.clone();
                            let channels = channels.clone();
                            let sensors = sensors.clone();
                            let bus = bus.clone();
                            // Process each event concurrently
                            tokio::spawn(async move {
                                if let Err(e) = process_event(event, &modules, &channels, &sensors, &bus).await {
                                    tracing::error!(error = ?e, "pipeline processing failed");
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
) -> anyhow::Result<()> {
    match event {
        ChannelEvent::TextMessage {
            channel_id,
            user_id,
            text,
        } => {
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
            )
            .await?;

            // Only call send_message when the text was NOT already streamed.
            if !result.streamed {
                channel.send_message(&target, &result.text).await?;
            }

            Ok(())
        }
        ChannelEvent::AudioInput {
            channel_id,
            user_id,
            audio,
        } => {
            // STT → Router → LLM → TTS
            let transcript = modules
                .stt
                .transcribe(&audio.data, AudioEncoding::Pcm16kMono)
                .await?;

            if transcript.trim().is_empty() {
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
            )
            .await?;

            // Text delivery: only if not already streamed
            if !result.streamed {
                channel.send_message(&target, &result.text).await?;
            }

            // Audio delivery: always send TTS regardless of streaming
            let audio_bytes = modules.tts.synthesize(&result.text).await?;
            let audio_out = AudioPayload {
                data: audio_bytes,
                sample_rate: 24000,
                channels: 1,
                format: crate::channels::adapter::AudioFormat::Pcm16,
            };

            if channel.capabilities().voice_output {
                channel.send_audio(&target, audio_out).await?;
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
        return Ok(TextResult {
            text: response_text,
            streamed: false,
        });
    }

    // 3. Route the input
    let route_input = RouterInput {
        text: text.to_string(),
        user_id: uid,
        conversation_context: history.clone(),
    };
    let decision = modules.router.route(&route_input).await?;

    let (response, streamed) = match decision {
        // Cache hit from router — never streamed
        RouteDecision::CacheHit { response } => (response, false),

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
                let full = stream_llm_response(
                    modules, channel, target, channel_id, user_id, event_bus, &messages,
                )
                .await?;
                (full, true)
            } else {
                let full = modules.llm.chat(&messages).await?;
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
                let full = stream_llm_response(
                    modules, channel, target, channel_id, user_id, event_bus, &messages,
                )
                .await?;
                (full, true)
            } else {
                let full = modules.llm.chat(&messages).await?;
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
) -> anyhow::Result<String> {
    let mut token_stream = modules.llm.chat_stream(messages).await?;

    channel.begin_stream(target).await?;

    let mut full_text = String::new();

    while let Some(chunk_result) = token_stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                full_text.push_str(&chunk);

                // Deliver to the channel
                if let Err(e) = channel.send_stream_chunk(target, &chunk).await {
                    tracing::warn!(error = ?e, "stream chunk delivery failed");
                }

                // Publish on event bus for WS subscribers
                event_bus.publish(ServerEvent::StreamChunk {
                    channel_id: channel_id.to_string(),
                    user_id: user_id.to_string(),
                    chunk,
                });
            }
            Err(e) => {
                tracing::error!(error = ?e, "LLM stream error");
                break;
            }
        }
    }

    channel.end_stream(target, &full_text).await?;

    event_bus.publish(ServerEvent::StreamEnd {
        channel_id: channel_id.to_string(),
        user_id: user_id.to_string(),
    });

    Ok(full_text)
}
