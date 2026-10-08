use super::debug::{debug_component_text, log_channel_delivery_failure};
use super::error_notice::{InteractionErrorStage, notify_interaction_error};
use super::expression::execute_expression_and_trace;
use super::generation::process_text;
use super::persistence::persist_text_interaction;
use super::policy::SharedInteractionPolicies;
use super::services::InteractionServices;
use super::turn::{VoiceTurn, VoiceTurnRegistry, cancellable_stage, turn_is_interrupted};
use super::voice_input::RecognizedVoiceInput;
use super::{delivery, supervision, voice};
use crate::channels::ChannelRegistry;
use crate::channels::adapter::{ChannelAdapter, ChannelEvent, ChannelTarget};
use crate::config::{
    CachePolicyResourceSection, CacheSection, ConversationSection, MemorySection, ToolsSection,
};
use crate::event::{EventBus, ServerEvent};
use crate::modules::platform::sensors::SensorRegistry;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Spawn the pipeline processing loop.
///
/// The task runs until the cancel token is triggered. It subscribes to the
/// EventBus, filters for `ChannelInput` events, processes them through the
/// AI pipeline, and sends responses back via the channel registry.
/// Starts the bounded request supervisor. The owner must await the returned
/// task before shutting down modules/storage; dropping it does not join workers.
pub fn spawn_interaction(
    event_bus: &EventBus,
    modules: Arc<InteractionServices>,
    channels: Arc<ChannelRegistry>,
    sensors: Arc<SensorRegistry>,
    cancel_token: CancellationToken,
    debug_output: bool,
    policies: SharedInteractionPolicies,
    mut voice_rx: tokio::sync::mpsc::Receiver<RecognizedVoiceInput>,
) -> tokio::task::JoinHandle<()> {
    let mut rx = event_bus.subscribe();
    let bus = event_bus.clone();
    let voice_turns = Arc::new(VoiceTurnRegistry::default());
    let shutdown = cancel_token.child_token();
    tokio::spawn(async move {
        let mut tasks = supervision::InteractionTasks::default();
        let mut pending = std::collections::VecDeque::new();
        let mut voice_open = true;
        loop {
            if shutdown.is_cancelled() {
                break;
            }
            // The reliable voice receiver keeps its own bounded queue. Channel
            // broadcast events have a separate bounded, best-effort queue.
            let next = tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tasks.join_next(), if !tasks.is_empty() => continue,
                input = voice_rx.recv(), if voice_open && tasks.has_capacity() => {
                    match input {
                        Some(input) if !input.cancel.is_cancelled() => Some(voice::InteractionInput::Recognized(input)),
                        Some(_) => continue,
                        None => { voice_open = false; continue; }
                    }
                }
                _ = std::future::ready(()), if tasks.has_capacity() && !pending.is_empty() => {
                    pending.pop_front().map(voice::InteractionInput::ChannelEvent)
                }
                event = rx.recv() => {
                    match event {
                        Ok(ServerEvent::ChannelInput(event)) => {
                            if pending.len() < supervision::CHANNEL_QUEUE_CAPACITY {
                                pending.push_back(event);
                            } else {
                                tracing::warn!(channel_id = channel_event_id(&event), "channel input rejected: queue full");
                            }
                            continue;
                        }
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                            tracing::warn!(missed, "channel event bus lagged");
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
                voice::InteractionInput::Recognized(input) => input.cancel.clone(),
                voice::InteractionInput::ChannelEvent(_) => shutdown.child_token(),
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
                // the worker before it touches context or an LLM.
                let policy = policies.read().await.clone();
                let result = match input {
                    voice::InteractionInput::ChannelEvent(event) => {
                        process_event(
                            event,
                            &modules,
                            &channels,
                            &sensors,
                            &bus,
                            worker_token,
                            debug_output,
                            &policy.conversation,
                            &policy.tools,
                            &policy.cache,
                            &policy.cache_policy,
                            &policy.memory,
                        )
                        .await
                    }
                    voice::InteractionInput::Recognized(input) => {
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
pub(super) async fn process_event(
    event: ChannelEvent,
    modules: &InteractionServices,
    channels: &ChannelRegistry,
    sensors: &SensorRegistry,
    event_bus: &EventBus,
    request_cancel: CancellationToken,
    debug_output: bool,
    conversation_config: &ConversationSection,
    tools_config: &ToolsSection,
    cache_config: &CacheSection,
    cache_policy: &CachePolicyResourceSection,
    memory_config: &MemorySection,
) -> anyhow::Result<()> {
    if request_cancel.is_cancelled() {
        return Ok(());
    }
    match event {
        ChannelEvent::TextMessage {
            channel_id,
            user_id,
            text,
            source_received_at,
            source_privacy_epoch,
            source_relationship_epoch,
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

            let mut turn = VoiceTurn::request(request_cancel);
            turn.output_channel = Some(channel.clone());
            turn.received_at = source_received_at;
            turn.source_privacy_epoch = Some(source_privacy_epoch);
            turn.source_relationship_epoch = Some(source_relationship_epoch);
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
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    notify_interaction_error(
                        modules,
                        channel.as_ref(),
                        &target,
                        &channel_id,
                        &user_id,
                        InteractionErrorStage::Llm,
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
            if !result.streamed && channel.capabilities().text {
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
                    delivery::text(modules, &target, Some(&turn), "text:response", &result.text),
                )
                .await
                {
                    log_channel_delivery_failure(channel.as_ref(), error).await;
                }
            }
            if let Some(plan) = &result.expression_plan {
                execute_expression_and_trace(
                    modules.expression.orchestrator(),
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
        ChannelEvent::SensorData {
            channel_id,
            sensor_type,
            payload,
        } => {
            let (channel_id, _channel) = resolve_event_channel(channels, &channel_id).await?;
            // Ingest sensor data into the registry
            use crate::modules::platform::sensors::SensorReading;
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

pub(super) fn channel_event_id(event: &ChannelEvent) -> Option<&str> {
    match event {
        ChannelEvent::TextMessage { channel_id, .. }
        | ChannelEvent::SensorData { channel_id, .. }
        | ChannelEvent::StatusChange { channel_id, .. } => Some(channel_id),
    }
}

pub(super) async fn resolve_event_channel(
    channels: &ChannelRegistry,
    channel_id: &str,
) -> anyhow::Result<(String, Arc<dyn ChannelAdapter>)> {
    channels
        .get(channel_id)
        .await
        .map(|channel| (channel_id.to_owned(), channel))
        .ok_or_else(|| anyhow::anyhow!("channel '{}' not found for response delivery", channel_id))
}
