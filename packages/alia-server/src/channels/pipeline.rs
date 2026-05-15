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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::channels::ChannelRegistry;
use crate::channels::adapter::{
    AudioFormat, AudioPayload, AudioPayloadStream, ChannelAdapter, ChannelEvent, ChannelTarget,
};
use crate::config::ToolsSection;
use crate::event::{EventBus, ServerEvent};
use crate::module::ModuleRegistry;
use crate::providers::adapter::{
    AudioEncoding, CachedResponse, ChatMessage, LlmToolCall, PromptVars, RouteDecision,
    RouterInput, ToolCall, ToolContext, ToolDefinition, ToolResult, TtsAdapter,
};
use crate::providers::sensor::SensorRegistry;

const TOOL_USE_SYSTEM_PROMPT: &str = r#"Tool use rules:
- When an appropriate tool is available, call the tool instead of describing imaginary results.
- Never put raw tool-call JSON, function names, or hidden arguments in the final user-facing answer.
- After tool results are provided, produce only a natural-language answer for the user.
- If a tool result says confirmation is required, ask the user to confirm before claiming the action was done.
- If native tool calling is unavailable, request tools by replying only with JSON:
  {"tool_calls":[{"name":"tool_name","arguments":{}}]}
"#;
const TTS_SENTENCE_MAX_CHARS: usize = 80;

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
                            // Process each event concurrently
                            tokio::spawn(async move {
                                if let Err(e) = process_event(event, &modules, &channels, &sensors, &bus, debug_output, &tools_config).await {
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
                channel.capabilities().voice_output && channel.capabilities().audio_streaming,
                tools_config,
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

            if channel.capabilities().voice_output && !result.audio_streamed {
                deliver_tts_audio(
                    modules,
                    channel.as_ref(),
                    &target,
                    &channel_id,
                    &user_id,
                    &result.text,
                    debug_output,
                )
                .await;
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

async fn deliver_tts_audio(
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    text: &str,
    debug_output: bool,
) {
    if channel.capabilities().audio_streaming {
        debug_component_text(
            debug_output,
            "tts",
            "stream_call",
            text,
            Some((channel_id, user_id)),
        );

        match modules.tts.synthesize_stream(text).await {
            Ok(tts_stream) => {
                let audio_started = Arc::new(AtomicBool::new(false));
                let audio_started_marker = audio_started.clone();
                let audio_stream = tts_stream.map(move |chunk| {
                    chunk
                        .map(|data| AudioPayload {
                            data,
                            sample_rate: 24000,
                            channels: 1,
                            format: AudioFormat::Wav,
                        })
                        .inspect(|_| {
                            audio_started_marker.store(true, Ordering::Relaxed);
                        })
                });

                if let Err(error) = channel
                    .send_audio_stream(target, Box::pin(audio_stream))
                    .await
                {
                    log_channel_delivery_failure(channel, error).await;
                    if audio_started.load(Ordering::Relaxed) {
                        return;
                    }
                    tracing::warn!(
                        channel_type = channel.channel_type(),
                        channel_id = channel.channel_id(),
                        "streaming TTS failed before audio playback; falling back to buffered TTS"
                    );
                } else {
                    return;
                }
            }
            Err(error) => {
                tracing::warn!(
                    channel_type = channel.channel_type(),
                    channel_id = channel.channel_id(),
                    error = %error,
                    "streaming TTS failed; falling back to buffered TTS"
                );
                tracing::debug!(error = ?error, "streaming TTS failure details");
            }
        }
    }

    debug_component_text(
        debug_output,
        "tts",
        "call",
        text,
        Some((channel_id, user_id)),
    );
    let audio_bytes = match modules.tts.synthesize(text).await {
        Ok(audio_bytes) => audio_bytes,
        Err(error) => {
            tracing::error!(
                channel_type = channel.channel_type(),
                channel_id = channel.channel_id(),
                error = %error,
                "TTS synthesis failed; keeping channel enabled"
            );
            tracing::debug!(error = ?error, "TTS synthesis failure details");
            return;
        }
    };
    debug_component_audio(
        debug_output,
        "tts",
        "result",
        audio_bytes.len(),
        Some((channel_id, user_id)),
    );

    let audio_out = AudioPayload {
        data: audio_bytes,
        sample_rate: 24000,
        channels: 1,
        format: AudioFormat::Wav,
    };

    debug_component_audio(
        debug_output,
        "channel",
        "send_audio",
        audio_out.data.len(),
        Some((channel_id, user_id)),
    );
    if let Err(error) = channel.send_audio(target, audio_out).await {
        log_channel_delivery_failure(channel, error).await;
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
    audio_response: bool,
    tools_config: &ToolsSection,
) -> anyhow::Result<TextResult> {
    let uid = Uuid::parse_str(user_id).unwrap_or_else(|_| Uuid::nil());
    let persona_prompt = build_persona_system_prompt(uid, modules).await?;

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
        RouteDecision::Skill { skill_name, params } => {
            let skill_match = crate::providers::adapter::SkillMatch {
                name: skill_name,
                confidence: 1.0,
                parameters: params.clone(),
            };
            let resolution = modules.skills.resolve(&skill_match).await?;

            // Build messages with the skill's domain-specific system prompt
            let mut messages = Vec::new();
            if let Some(sp) = persona_prompt.as_ref() {
                messages.push(ChatMessage {
                    role: "system".into(),
                    content: sp.clone(),
                    name: None,
                    ..ChatMessage::default()
                });
            }
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
                let full = modules.llm.chat(&messages).await?;
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

        // LLM call — stream when the channel supports it
        RouteDecision::LocalLlm { system_prompt } | RouteDecision::RemoteLlm { system_prompt } => {
            let mut messages = Vec::new();
            if let Some(sp) = persona_prompt.as_ref() {
                messages.push(ChatMessage {
                    role: "system".into(),
                    content: sp.clone(),
                    name: None,
                    ..ChatMessage::default()
                });
            }
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

            if channel.capabilities().streaming {
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
                let full = modules.llm.chat(&messages).await?;
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
                content: response.clone(),
                name: None,
                ..ChatMessage::default()
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
        audio_streamed,
    })
}

async fn run_llm_tool_loop(
    modules: &ModuleRegistry,
    base_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    user_id: Uuid,
    session_id: Option<Uuid>,
    channel_id: &str,
    user_id_text: &str,
    route: &'static str,
    tools_config: &ToolsSection,
    debug_output: bool,
) -> anyhow::Result<String> {
    let mut messages = inject_tool_prompt(base_messages, &tools)?;
    let tool_map = tools
        .iter()
        .map(|tool| (tool.name.clone(), tool.clone()))
        .collect::<HashMap<_, _>>();
    let ctx = ToolContext {
        user_id,
        conversation_id: session_id,
        source: "ai".to_string(),
    };
    let max_depth = tools_config.max_tool_call_depth.max(1);

    for depth in 0..max_depth {
        debug_component_llm_call(
            debug_output,
            "llm",
            "tool_call",
            route,
            &messages,
            Some((channel_id, user_id_text)),
        );

        let mut response = modules.llm.chat_with_tools(&messages, &tools).await?;
        if response.tool_calls.is_empty() {
            response.tool_calls = parse_text_tool_calls(&response.content)?;
        }

        if response.tool_calls.is_empty() {
            return Ok(response.content);
        }

        debug_component_tool_calls(
            debug_output,
            depth + 1,
            &response.tool_calls,
            Some((channel_id, user_id_text)),
        );

        messages.push(ChatMessage {
            role: "assistant".into(),
            content: response.content.clone(),
            name: None,
            tool_calls: response.tool_calls.clone(),
            ..ChatMessage::default()
        });

        for (index, call) in response.tool_calls.iter().enumerate() {
            let call = normalize_tool_call(call, depth, index);
            let result =
                execute_pipeline_tool_call(modules, &call, &tool_map, tools_config, &ctx).await;
            debug_component_tool_result(
                debug_output,
                &call.tool_name,
                &result,
                Some((channel_id, user_id_text)),
            );
            messages.push(tool_result_message(&call.tool_name, result)?);
        }
    }

    tracing::warn!(
        depth = max_depth,
        "LLM tool loop reached max_tool_call_depth; asking model for final answer"
    );
    messages.push(ChatMessage::new(
        "system",
        "Tool-call depth limit reached. Stop calling tools and give the user a concise final answer based on the tool results already available.",
    ));

    let final_response = modules.llm.chat(&messages).await?;
    Ok(final_response)
}

async fn select_tools_for_skill(
    modules: &ModuleRegistry,
    allowed_names: &[String],
    tools_config: &ToolsSection,
) -> anyhow::Result<Vec<ToolDefinition>> {
    let all_tools = modules.tools.list_tools().await?;
    let allowed = (!allowed_names.is_empty()).then(|| {
        allowed_names
            .iter()
            .map(|name| name.as_str())
            .collect::<HashSet<_>>()
    });

    let tools = all_tools
        .into_iter()
        .filter(|tool| {
            if !tools_config.allow_plugin_tools && tool.source != "builtin" {
                return false;
            }
            allowed
                .as_ref()
                .is_none_or(|allowed| allowed.contains(tool.name.as_str()))
        })
        .collect::<Vec<_>>();

    if let Some(allowed) = allowed {
        let found = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<HashSet<_>>();
        for name in allowed.difference(&found) {
            tracing::warn!(tool = %name, "skill requested unavailable tool");
        }
    }

    Ok(tools)
}

fn inject_tool_prompt(
    mut messages: Vec<ChatMessage>,
    tools: &[ToolDefinition],
) -> anyhow::Result<Vec<ChatMessage>> {
    let tool_summary = tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
                "requires_confirmation": tool.require_confirmation,
            })
        })
        .collect::<Vec<_>>();
    let prompt = format!(
        "{TOOL_USE_SYSTEM_PROMPT}\nAvailable tools:\n{}",
        serde_json::to_string_pretty(&tool_summary)?
    );

    let insert_at = messages
        .iter()
        .rposition(|message| message.role == "system")
        .map(|index| index + 1)
        .unwrap_or(0);
    messages.insert(insert_at, ChatMessage::new("system", prompt));
    Ok(messages)
}

async fn execute_pipeline_tool_call(
    modules: &ModuleRegistry,
    call: &LlmToolCall,
    tool_map: &HashMap<String, ToolDefinition>,
    tools_config: &ToolsSection,
    ctx: &ToolContext,
) -> ToolResult {
    let Some(definition) = tool_map.get(&call.tool_name) else {
        return ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: serde_json::json!({
                "error": "tool_not_available",
                "tool": call.tool_name,
            }),
            display: Some(format!("Tool '{}' is not available", call.tool_name)),
        };
    };

    if tool_requires_confirmation(definition, tools_config) {
        return ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: serde_json::json!({
                "requires_confirmation": true,
                "tool": call.tool_name,
                "arguments": call.arguments.clone(),
                "message": "This tool requires user confirmation before execution.",
            }),
            display: Some(format!("Please confirm before I run '{}'.", call.tool_name)),
        };
    }

    let tool_call = ToolCall {
        tool_name: call.tool_name.clone(),
        arguments: call.arguments.clone(),
        call_id: call.id.clone(),
    };
    let timeout = Duration::from_secs(tools_config.tool_call_timeout_sec.max(1));

    match tokio::time::timeout(timeout, modules.tools.execute(tool_call, ctx)).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: serde_json::json!({
                "error": error.to_string(),
            }),
            display: Some(format!("Tool '{}' failed: {}", call.tool_name, error)),
        },
        Err(_) => ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: serde_json::json!({
                "error": "tool_timeout",
                "timeout_sec": tools_config.tool_call_timeout_sec,
            }),
            display: Some(format!("Tool '{}' timed out", call.tool_name)),
        },
    }
}

fn tool_requires_confirmation(tool: &ToolDefinition, tools_config: &ToolsSection) -> bool {
    tool.require_confirmation
        || tools_config
            .require_confirmation
            .iter()
            .any(|name| name == &tool.name)
}

fn normalize_tool_call(call: &LlmToolCall, depth: u32, index: usize) -> LlmToolCall {
    let id = if call.id.trim().is_empty() {
        format!("call_{depth}_{index}")
    } else {
        call.id.clone()
    };
    LlmToolCall {
        id,
        tool_name: call.tool_name.clone(),
        arguments: call.arguments.clone(),
    }
}

fn tool_result_message(tool_name: &str, result: ToolResult) -> anyhow::Result<ChatMessage> {
    let content = serde_json::to_string(&serde_json::json!({
        "tool": tool_name,
        "success": result.success,
        "output": result.output,
        "display": result.display,
    }))?;

    Ok(ChatMessage {
        role: "tool".into(),
        content,
        name: Some(tool_name.to_string()),
        tool_call_id: Some(result.call_id),
        ..ChatMessage::default()
    })
}

fn parse_text_tool_calls(content: &str) -> anyhow::Result<Vec<LlmToolCall>> {
    let candidate = strip_tool_call_json(content);
    if !candidate.contains("\"tool_calls\"") {
        return Ok(Vec::new());
    }

    let value: serde_json::Value = serde_json::from_str(&candidate)
        .map_err(|error| anyhow::anyhow!("failed to parse text tool call JSON: {error}"))?;
    let Some(calls) = value.get("tool_calls").and_then(|value| value.as_array()) else {
        return Ok(Vec::new());
    };

    let parsed = calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let name = call
                .get("name")
                .or_else(|| call.get("tool_name"))
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("tool call {index} missing name"))?;
            let arguments = call
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let arguments = match arguments {
                serde_json::Value::String(raw) => serde_json::from_str(&raw).map_err(|error| {
                    anyhow::anyhow!("tool call {index} invalid arguments: {error}")
                })?,
                value => value,
            };
            let id = call
                .get("id")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned)
                .unwrap_or_default();
            Ok(LlmToolCall {
                id,
                tool_name: name.to_string(),
                arguments,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(parsed)
}

fn strip_tool_call_json(content: &str) -> String {
    let trimmed = content.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .trim();
    let trimmed = trimmed.strip_suffix("```").unwrap_or(trimmed).trim();

    if trimmed.starts_with('{') {
        return trimmed.to_string();
    }

    match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(start), Some(end)) if start < end => trimmed[start..=end].to_string(),
        _ => trimmed.to_string(),
    }
}

async fn build_persona_system_prompt(
    user_id: Uuid,
    modules: &ModuleRegistry,
) -> anyhow::Result<Option<String>> {
    let available_skills = modules
        .skills
        .list_skills()
        .await
        .map(|skills| skills.into_iter().map(|skill| skill.display_name).collect())
        .unwrap_or_default();

    let vars = PromptVars {
        current_time: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        available_skills,
        ..PromptVars::default()
    };

    match modules.persona.render_system_prompt(user_id, vars).await {
        Ok(prompt) if !prompt.trim().is_empty() => Ok(Some(prompt)),
        Ok(_) => Ok(None),
        Err(error) => {
            tracing::warn!(user_id = %user_id, error = ?error, "failed to render persona prompt");
            Ok(None)
        }
    }
}

async fn stream_llm_response_with_sentence_tts(
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    event_bus: &EventBus,
    messages: &[ChatMessage],
    debug_output: bool,
) -> anyhow::Result<(String, bool)> {
    let (sentence_tx, sentence_rx) = mpsc::channel::<String>(4);
    let (audio_tx, audio_rx) = mpsc::channel::<anyhow::Result<AudioPayload>>(8);
    let audio_started = Arc::new(AtomicBool::new(false));
    let tts_worker = tokio::spawn(stream_tts_sentences(
        modules.tts.clone(),
        sentence_rx,
        audio_tx,
        audio_started.clone(),
        debug_output,
        channel_id.to_string(),
        user_id.to_string(),
    ));

    let audio_stream: AudioPayloadStream =
        Box::pin(tokio_stream::wrappers::ReceiverStream::new(audio_rx));
    let audio_delivery = channel.send_audio_stream(target, audio_stream);

    let llm_delivery = async move {
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
        let mut sentence_buffer = String::new();
        let mut sentence_tx = Some(sentence_tx);

        while let Some(chunk_result) = token_stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    full_text.push_str(&chunk);
                    sentence_buffer.push_str(&chunk);
                    debug_component_stream_chunk(debug_output, &chunk, Some((channel_id, user_id)));

                    if let Err(e) = channel.send_stream_chunk(target, &chunk).await {
                        tracing::warn!(error = %e, "stream chunk delivery failed");
                        tracing::debug!(error = ?e, "stream chunk delivery failure details");
                    }

                    event_bus.publish(ServerEvent::StreamChunk {
                        channel_id: channel_id.to_string(),
                        user_id: user_id.to_string(),
                        chunk,
                    });

                    while let Some(segment) = drain_speakable_segment(&mut sentence_buffer, false) {
                        let Some(tx) = sentence_tx.as_ref() else {
                            break;
                        };
                        if tx.send(segment).await.is_err() {
                            sentence_tx = None;
                            break;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "LLM stream error");
                    tracing::debug!(error = ?e, "LLM stream error details");
                    break;
                }
            }
        }

        while let Some(segment) = drain_speakable_segment(&mut sentence_buffer, true) {
            let Some(tx) = sentence_tx.as_ref() else {
                break;
            };
            if tx.send(segment).await.is_err() {
                sentence_tx = None;
                break;
            }
        }
        drop(sentence_tx);

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

        Ok::<_, anyhow::Error>(full_text)
    };

    let (llm_result, audio_result) = futures::join!(llm_delivery, audio_delivery);
    let tts_result = tts_worker
        .await
        .map_err(|error| anyhow::anyhow!("sentence TTS worker join failed: {error}"));
    let audio_streamed = audio_started.load(Ordering::Relaxed);

    if let Err(error) = audio_result {
        tracing::warn!(
            channel_type = channel.channel_type(),
            channel_id = channel.channel_id(),
            error = %error,
            "sentence-level TTS audio delivery failed"
        );
        tracing::debug!(error = ?error, "sentence-level TTS audio delivery failure details");
    }
    if let Err(error) = tts_result.and_then(|result| result) {
        tracing::warn!(error = %error, "sentence-level TTS worker failed");
        tracing::debug!(error = ?error, "sentence-level TTS worker failure details");
    }

    Ok((llm_result?, audio_streamed))
}

async fn stream_tts_sentences(
    tts: Arc<dyn TtsAdapter>,
    mut sentence_rx: mpsc::Receiver<String>,
    audio_tx: mpsc::Sender<anyhow::Result<AudioPayload>>,
    audio_started: Arc<AtomicBool>,
    debug_output: bool,
    channel_id: String,
    user_id: String,
) -> anyhow::Result<()> {
    while let Some(sentence) = sentence_rx.recv().await {
        let sentence = sentence.trim();
        if sentence.is_empty() {
            continue;
        }

        debug_component_text(
            debug_output,
            "tts",
            "sentence_stream_call",
            sentence,
            Some((&channel_id, &user_id)),
        );
        let mut stream = match tts.synthesize_stream(sentence).await {
            Ok(stream) => stream,
            Err(error) => {
                let message = format!("TTS stream setup failed: {error}");
                let _ = audio_tx.send(Err(anyhow::anyhow!(message))).await;
                return Err(error);
            }
        };

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(data) => {
                    audio_started.store(true, Ordering::Relaxed);
                    let payload = AudioPayload {
                        data,
                        sample_rate: 24000,
                        channels: 1,
                        format: AudioFormat::Wav,
                    };
                    if audio_tx.send(Ok(payload)).await.is_err() {
                        return Ok(());
                    }
                }
                Err(error) => {
                    let message = format!("TTS stream chunk failed: {error}");
                    let _ = audio_tx.send(Err(anyhow::anyhow!(message))).await;
                    return Err(error);
                }
            }
        }
    }

    Ok(())
}

fn drain_speakable_segment(buffer: &mut String, final_flush: bool) -> Option<String> {
    let sentence_boundary = buffer
        .char_indices()
        .find(|(_, ch)| matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | '\n'))
        .map(|(idx, ch)| idx + ch.len_utf8());

    let boundary = sentence_boundary.or_else(|| {
        if final_flush {
            return Some(buffer.len());
        }
        if buffer.chars().count() < TTS_SENTENCE_MAX_CHARS {
            return None;
        }

        let max_byte = buffer
            .char_indices()
            .nth(TTS_SENTENCE_MAX_CHARS)
            .map(|(idx, _)| idx)
            .unwrap_or(buffer.len());
        buffer[..max_byte]
            .char_indices()
            .rev()
            .find(|(_, ch)| matches!(ch, ',' | ';' | ':' | '，' | '；' | '：' | ' '))
            .map(|(idx, ch)| idx + ch.len_utf8())
            .or(Some(max_byte))
    })?;

    let segment = buffer[..boundary].trim().to_string();
    let rest = buffer[boundary..].to_string();
    *buffer = rest;

    if segment.is_empty() {
        None
    } else {
        Some(segment)
    }
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

fn debug_component_tools(
    enabled: bool,
    component: &'static str,
    event: &'static str,
    tools: &[ToolDefinition],
    channel: Option<(&str, &str)>,
) {
    if !enabled {
        tracing::debug!(
            target: "alia_server::debug_output",
            component,
            event,
            tools = %tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>().join(","),
            "component tools"
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
            tools = %tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>().join(","),
            "component tools"
        ),
        None => tracing::info!(
            target: "alia_server::debug_output",
            component,
            event,
            tools = %tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>().join(","),
            "component tools"
        ),
    }
}

fn debug_component_tool_calls(
    enabled: bool,
    depth: u32,
    calls: &[LlmToolCall],
    channel: Option<(&str, &str)>,
) {
    if !enabled {
        tracing::debug!(
            target: "alia_server::debug_output",
            component = "tools",
            event = "calls",
            depth,
            calls = %calls.iter().map(|call| call.tool_name.as_str()).collect::<Vec<_>>().join(","),
            "component tool calls"
        );
        return;
    }

    match channel {
        Some((channel_id, user_id)) => tracing::info!(
            target: "alia_server::debug_output",
            component = "tools",
            event = "calls",
            channel_id,
            user_id,
            depth,
            calls = %calls.iter().map(|call| call.tool_name.as_str()).collect::<Vec<_>>().join(","),
            "component tool calls"
        ),
        None => tracing::info!(
            target: "alia_server::debug_output",
            component = "tools",
            event = "calls",
            depth,
            calls = %calls.iter().map(|call| call.tool_name.as_str()).collect::<Vec<_>>().join(","),
            "component tool calls"
        ),
    }
}

fn debug_component_tool_result(
    enabled: bool,
    tool_name: &str,
    result: &ToolResult,
    channel: Option<(&str, &str)>,
) {
    if !enabled {
        tracing::debug!(
            target: "alia_server::debug_output",
            component = "tools",
            event = "result",
            tool = tool_name,
            success = result.success,
            display = %result.display.as_deref().unwrap_or(""),
            "component tool result"
        );
        return;
    }

    match channel {
        Some((channel_id, user_id)) => tracing::info!(
            target: "alia_server::debug_output",
            component = "tools",
            event = "result",
            channel_id,
            user_id,
            tool = tool_name,
            success = result.success,
            display = %result.display.as_deref().unwrap_or(""),
            "component tool result"
        ),
        None => tracing::info!(
            target: "alia_server::debug_output",
            component = "tools",
            event = "result",
            tool = tool_name,
            success = result.success,
            display = %result.display.as_deref().unwrap_or(""),
            "component tool result"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_tool_call_envelope() {
        let calls = parse_text_tool_calls(
            r#"{"tool_calls":[{"name":"get_current_time","arguments":{"format":"human"}}]}"#,
        )
        .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_name, "get_current_time");
        assert_eq!(calls[0].arguments["format"], "human");
    }

    #[test]
    fn parses_fenced_text_tool_call_envelope() {
        let calls = parse_text_tool_calls(
            "```json\n{\"tool_calls\":[{\"tool_name\":\"sensor_list\",\"arguments\":\"{}\"}]}\n```",
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
}
