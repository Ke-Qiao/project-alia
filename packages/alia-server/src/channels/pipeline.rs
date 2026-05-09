//! Channel → AI Pipeline integration.
//!
//! Subscribes to `ServerEvent::ChannelInput` on the EventBus, routes each
//! inbound `ChannelEvent` through: Router → Cache/Skill/LLM → ChannelResponse,
//! then delivers the response back to the originating channel.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::channels::adapter::{
    AudioPayload, ChannelEvent, ChannelTarget,
};
use crate::channels::ChannelRegistry;
use crate::event::{EventBus, ServerEvent};
use crate::module::ModuleRegistry;
use crate::providers::adapter::{
    AudioEncoding, CachedResponse, ChatMessage, RouteDecision, RouterInput,
};

/// Spawn the pipeline processing loop.
///
/// The task runs until the cancel token is triggered. It subscribes to the
/// EventBus, filters for `ChannelInput` events, processes them through the
/// AI pipeline, and sends responses back via the channel registry.
pub fn spawn_pipeline(
    event_bus: &EventBus,
    modules: Arc<ModuleRegistry>,
    channels: Arc<ChannelRegistry>,
    cancel_token: CancellationToken,
) {
    let mut rx = event_bus.subscribe();

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
                            // Process each event concurrently
                            tokio::spawn(async move {
                                if let Err(e) = process_event(event, &modules, &channels).await {
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
) -> anyhow::Result<()> {
    match event {
        ChannelEvent::TextMessage {
            channel_id,
            user_id,
            text,
        } => {
            let response_text = process_text(&text, &user_id, modules).await?;
            send_response(channels, &channel_id, &user_id, Some(response_text), None).await
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

            let response_text = process_text(&transcript, &user_id, modules).await?;

            // Synthesize TTS
            let audio_bytes = modules.tts.synthesize(&response_text).await?;
            let audio_out = AudioPayload {
                data: audio_bytes,
                sample_rate: 24000,
                channels: 1,
                format: crate::channels::adapter::AudioFormat::Pcm16,
            };

            send_response(
                channels,
                &channel_id,
                &user_id,
                Some(response_text),
                Some(audio_out),
            )
            .await
        }
        ChannelEvent::SensorData { .. } | ChannelEvent::StatusChange { .. } => {
            // Sensor and status events don't go through the LLM pipeline
            Ok(())
        }
    }
}

/// Core text processing: Router → Cache/Skill/LLM → response text.
async fn process_text(
    text: &str,
    user_id: &str,
    modules: &ModuleRegistry,
) -> anyhow::Result<String> {
    let uid = Uuid::parse_str(user_id).unwrap_or_else(|_| Uuid::nil());

    // 1. Check cache first
    if let Some(CachedResponse { response_text, .. }) =
        modules.cache.get(Some(uid), text).await?
    {
        tracing::debug!(query = text, "pipeline cache hit");
        return Ok(response_text);
    }

    // 2. Route the input
    let route_input = RouterInput {
        text: text.to_string(),
        user_id: uid,
        conversation_context: Vec::new(),
    };
    let decision = modules.router.route(&route_input).await?;

    let response = match decision {
        RouteDecision::CacheHit { response } => response,
        RouteDecision::Skill { skill_name, params } => {
            let skill_match = crate::providers::adapter::SkillMatch {
                name: skill_name,
                confidence: 1.0,
                parameters: params.clone(),
            };
            let result = modules.skills.execute(&skill_match, params).await?;
            result.message.unwrap_or_else(|| {
                serde_json::to_string_pretty(&result.output).unwrap_or_default()
            })
        }
        RouteDecision::LocalLlm { system_prompt } | RouteDecision::RemoteLlm { system_prompt } => {
            let mut messages = Vec::new();
            if let Some(sp) = system_prompt {
                messages.push(ChatMessage {
                    role: "system".into(),
                    content: sp,
                });
            }
            messages.push(ChatMessage {
                role: "user".into(),
                content: text.to_string(),
            });
            modules.llm.chat(&messages).await?
        }
    };

    // 3. Store in cache for next time
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

    Ok(response)
}

/// Deliver a response back to the originating channel.
async fn send_response(
    channels: &ChannelRegistry,
    channel_id: &str,
    user_id: &str,
    text: Option<String>,
    audio: Option<AudioPayload>,
) -> anyhow::Result<()> {
    let channel = channels
        .get(channel_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("channel '{}' not found for response delivery", channel_id))?;

    let target = ChannelTarget {
        user_id: user_id.to_string(),
    };

    // Send text response
    if let Some(ref msg) = text {
        channel.send_message(&target, msg).await?;
    }

    // Send audio if channel supports it and we have audio
    if let Some(audio_payload) = audio {
        if channel.capabilities().voice_output {
            channel.send_audio(&target, audio_payload).await?;
        }
    }

    Ok(())
}
