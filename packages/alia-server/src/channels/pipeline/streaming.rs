use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::debug::{
    debug_component, debug_component_audio, debug_component_json, debug_component_stream_chunk,
    debug_component_text, log_channel_delivery_failure,
};
use super::turn::{VoiceTurn, cancellable_stage, turn_is_interrupted};
use crate::channels::adapter::{
    AudioFormat, AudioPayload, AudioPayloadStream, ChannelAdapter, ChannelTarget,
};
use crate::config::SentenceExpressionLlmPolicySection;
use crate::event::{EventBus, ServerEvent};
use crate::expression::orchestrator::{ExpressionExecutionPhase, execute_expression_plan_phase};
use crate::expression::{
    ExpressionConstraints, ExpressionConversationContext, ExpressionDeviceContext, ExpressionInput,
    ExpressionPlan, ExpressionSafetyContext, ExpressionUserState, SpeechExpression,
};
use crate::module::ModuleRegistry;
use crate::providers::adapter::{
    ChatMessage, ConversationBackend, ConversationEventEntry, LlmAdapter, MemoryContextPack,
    TtsAdapter, TtsCapabilities, TtsRequest,
};
use uuid::Uuid;

use super::AgentTrace;
use super::error_notice::PipelineErrorStage;

const TTS_SENTENCE_MAX_CHARS: usize = 80;
const TTS_SENTENCE_GAP_MS: u32 = 80;
const SENTENCE_EXPRESSION_CACHE_CAPACITY: usize = 512;

static SENTENCE_EXPRESSION_CACHE: OnceLock<Mutex<HashMap<String, SpeechExpression>>> =
    OnceLock::new();

#[derive(Clone)]
pub(super) struct TtsTraceContext {
    session_id: Uuid,
    turn_id: Uuid,
    sequence: Arc<AtomicI32>,
    conversation: Arc<dyn ConversationBackend>,
}

impl TtsTraceContext {
    pub(super) fn from_agent_trace(modules: &ModuleRegistry, trace: &AgentTrace) -> Self {
        Self {
            session_id: trace.session_id,
            turn_id: trace.turn_id,
            sequence: Arc::new(AtomicI32::new(10_000)),
            conversation: modules.conversation.clone(),
        }
    }

    async fn record(
        &self,
        event_type: impl Into<String>,
        actor: impl Into<String>,
        summary: Option<String>,
        payload: serde_json::Value,
    ) {
        let event = ConversationEventEntry {
            turn_id: self.turn_id,
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            event_type: event_type.into(),
            actor: actor.into(),
            summary,
            payload,
        };
        if let Err(error) = self.conversation.append_event(self.session_id, event).await {
            tracing::debug!(error = ?error, "failed to append TTS trace event");
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct SentenceSpeechContext {
    pub route: Option<String>,
    pub channel_id: String,
    pub user_id: String,
    pub dialogue_act: String,
    pub content_tone: String,
    pub user_state: ExpressionUserState,
    pub device_context: ExpressionDeviceContext,
    pub safety_context: ExpressionSafetyContext,
    pub conversation_context: ExpressionConversationContext,
    pub constraints: ExpressionConstraints,
    pub memory_context: Option<MemoryContextPack>,
    pub tts_capabilities: Option<TtsCapabilities>,
}

impl SentenceSpeechContext {
    fn input_for_sentence(&self, sentence: &str) -> ExpressionInput {
        ExpressionInput {
            response_text: sentence.to_string(),
            route: self.route.clone(),
            channel_id: self.channel_id.clone(),
            user_id: self.user_id.clone(),
            dialogue_act: self.dialogue_act.clone(),
            content_tone: self.content_tone.clone(),
            user_state: self.user_state.clone(),
            device_context: self.device_context.clone(),
            safety_context: self.safety_context.clone(),
            conversation_context: ExpressionConversationContext {
                turn_kind: "streaming_sentence_tts".to_string(),
                ..self.conversation_context.clone()
            },
            constraints: self.constraints.clone(),
            memory_context: self.memory_context.clone(),
            voice_output: true,
            hardware_control: false,
            tts_capabilities: self.tts_capabilities.clone(),
            expression_capabilities: None,
        }
    }
}

pub(super) async fn deliver_tts_audio(
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    text: &str,
    speech: Option<&SpeechExpression>,
    expression_plan: Option<&ExpressionPlan>,
    expression_input: Option<&ExpressionInput>,
    trace_context: Option<TtsTraceContext>,
    debug_output: bool,
    turn: Option<&VoiceTurn>,
) -> TtsDeliveryOutcome {
    cancellable_stage(turn, "tts_output", async {
        Ok(deliver_tts_audio_inner(
            modules,
            channel,
            target,
            channel_id,
            user_id,
            text,
            speech,
            expression_plan,
            expression_input,
            trace_context,
            debug_output,
            turn,
        )
        .await)
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(TtsDeliveryOutcome::Skipped)
}

pub(super) async fn deliver_tts_audio_inner(
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    text: &str,
    speech: Option<&SpeechExpression>,
    expression_plan: Option<&ExpressionPlan>,
    expression_input: Option<&ExpressionInput>,
    trace_context: Option<TtsTraceContext>,
    debug_output: bool,
    turn: Option<&VoiceTurn>,
) -> TtsDeliveryOutcome {
    if turn_is_interrupted(turn) {
        return TtsDeliveryOutcome::Skipped;
    }
    if text.trim().is_empty() {
        tracing::warn!(
            channel_type = channel.channel_type(),
            channel_id = channel.channel_id(),
            "skipping TTS for empty response text"
        );
        return TtsDeliveryOutcome::Skipped;
    }

    let resolved_speech = if let (Some(input), Some(speech)) = (expression_input, speech) {
        Some(
            resolve_sentence_speech(
                modules.expression.clone(),
                modules.sentence_expression_llm.clone(),
                input.clone(),
                speech.clone(),
                trace_context.clone(),
                debug_output,
                channel_id,
                user_id,
            )
            .await,
        )
    } else {
        speech.cloned()
    };

    if channel.capabilities().audio_streaming {
        debug_component_text(
            debug_output,
            "tts",
            "stream_call",
            text,
            Some((channel_id, user_id)),
        );
        let tts_request = resolved_speech
            .as_ref()
            .map(|speech| TtsRequest::with_speech(text, speech))
            .unwrap_or_else(|| TtsRequest::plain(text));
        trace_tts_provider_adjustments(modules, debug_output, channel_id, user_id, &tts_request)
            .await;

        match cancellable_stage(
            turn,
            "tts_stream_start",
            modules.tts.synthesize_stream_request(&tts_request),
        )
        .await
        {
            Ok(Some(tts_stream)) => {
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

                let cancel_token = turn.map(|turn| turn.token.clone());
                execute_speech_start_expression(
                    channel,
                    target,
                    expression_plan,
                    debug_output,
                    channel_id,
                    user_id,
                    turn,
                )
                .await;
                let delivery = channel.send_audio_stream(target, Box::pin(audio_stream));
                let delivery_result = if let Some(cancel_token) = cancel_token {
                    tokio::select! {
                    biased;
                            _ = cancel_token.cancelled() => return TtsDeliveryOutcome::Skipped,
                            result = delivery => result,
                        }
                } else {
                    delivery.await
                };

                if let Err(error) = delivery_result {
                    log_channel_delivery_failure(channel, error).await;
                    if audio_started.load(Ordering::Relaxed) {
                        return TtsDeliveryOutcome::Failed(PipelineErrorStage::Delivery);
                    }
                    tracing::warn!(
                        channel_type = channel.channel_type(),
                        channel_id = channel.channel_id(),
                        "streaming TTS failed before audio playback; falling back to buffered TTS"
                    );
                } else {
                    return TtsDeliveryOutcome::Delivered;
                }
            }
            Ok(None) => return TtsDeliveryOutcome::Skipped,
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
    let tts_request = resolved_speech
        .as_ref()
        .map(|speech| TtsRequest::with_speech(text, speech))
        .unwrap_or_else(|| TtsRequest::plain(text));
    trace_tts_provider_adjustments(modules, debug_output, channel_id, user_id, &tts_request).await;
    let audio_bytes =
        match cancellable_stage(turn, "tts", modules.tts.synthesize_request(&tts_request)).await {
            Ok(Some(audio_bytes)) => audio_bytes,
            Ok(None) => return TtsDeliveryOutcome::Skipped,
            Err(error) => {
                tracing::error!(
                    channel_type = channel.channel_type(),
                    channel_id = channel.channel_id(),
                    error = %error,
                    "TTS synthesis failed; keeping channel enabled"
                );
                tracing::debug!(error = ?error, "TTS synthesis failure details");
                return TtsDeliveryOutcome::Failed(PipelineErrorStage::Tts);
            }
        };
    if turn_is_interrupted(turn) {
        return TtsDeliveryOutcome::Skipped;
    }
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

    execute_speech_start_expression(
        channel,
        target,
        expression_plan,
        debug_output,
        channel_id,
        user_id,
        turn,
    )
    .await;

    debug_component_audio(
        debug_output,
        "channel",
        "send_audio",
        audio_out.data.len(),
        Some((channel_id, user_id)),
    );
    let send_result = if let Some(turn) = turn {
        tokio::select! {
                biased;
            _ = turn.token.cancelled() => return TtsDeliveryOutcome::Skipped,
            result = channel.send_audio(target, audio_out) => result,
        }
    } else {
        channel.send_audio(target, audio_out).await
    };
    if let Err(error) = send_result {
        log_channel_delivery_failure(channel, error).await;
        return TtsDeliveryOutcome::Failed(PipelineErrorStage::Delivery);
    }
    TtsDeliveryOutcome::Delivered
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TtsDeliveryOutcome {
    Delivered,
    Skipped,
    Failed(PipelineErrorStage),
}

async fn trace_tts_provider_adjustments(
    modules: &ModuleRegistry,
    debug_output: bool,
    channel_id: &str,
    user_id: &str,
    request: &TtsRequest,
) {
    let adjustments = modules.tts.provider_adjustments(request).await;
    if adjustments.is_empty() {
        return;
    }
    debug_component_text(
        debug_output,
        "tts",
        "provider_adjustments",
        &serde_json::to_string(&adjustments).unwrap_or_else(|_| "[]".to_string()),
        Some((channel_id, user_id)),
    );
}

async fn execute_speech_start_expression(
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    plan: Option<&ExpressionPlan>,
    debug_output: bool,
    channel_id: &str,
    user_id: &str,
    turn: Option<&VoiceTurn>,
) {
    let Some(plan) = plan else {
        return;
    };
    let execution = cancellable_stage(turn, "speech_start_expression", async {
        Ok(execute_expression_plan_phase(
            channel,
            target,
            plan,
            ExpressionExecutionPhase::SpeechStart,
        )
        .await)
    })
    .await
    .ok()
    .flatten();
    debug_component_json(
        debug_output,
        "expression",
        ExpressionExecutionPhase::SpeechStart.label(),
        &json!({
            "plan_id": plan.id,
            "timeline_actions": plan.timeline.len(),
            "execution": execution,
        }),
        Some((channel_id, user_id)),
    );
}

pub(super) async fn stream_llm_response_with_sentence_tts(
    modules: &ModuleRegistry,
    llm: &dyn LlmAdapter,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    event_bus: &EventBus,
    messages: &[ChatMessage],
    speech_context: Option<SentenceSpeechContext>,
    trace_context: Option<TtsTraceContext>,
    debug_output: bool,
    turn: Option<&VoiceTurn>,
) -> anyhow::Result<(String, bool)> {
    let (sentence_tx, sentence_rx) = mpsc::channel::<String>(4);
    let (audio_tx, audio_rx) = mpsc::channel::<anyhow::Result<AudioPayload>>(8);
    let audio_started = Arc::new(AtomicBool::new(false));
    let tts_cancel_token = turn.map(|turn| turn.token.clone());
    let tts_worker = stream_tts_sentences(
        modules.tts.clone(),
        modules.expression.clone(),
        modules.sentence_expression_llm.clone(),
        sentence_rx,
        audio_tx,
        audio_started.clone(),
        speech_context,
        trace_context,
        debug_output,
        channel_id.to_string(),
        user_id.to_string(),
        tts_cancel_token.clone(),
    );

    let audio_stream: AudioPayloadStream =
        Box::pin(tokio_stream::wrappers::ReceiverStream::new(audio_rx));
    let audio_delivery = async move {
        if let Some(cancel_token) = tts_cancel_token {
            tokio::select! {
                biased;
                _ = cancel_token.cancelled() => Ok(()),
                result = channel.send_audio_stream(target, audio_stream) => result,
            }
        } else {
            channel.send_audio_stream(target, audio_stream).await
        }
    };

    let llm_delivery = async move {
        let mut token_stream =
            match cancellable_stage(turn, "llm_stream_start", llm.chat_stream(messages)).await? {
                Some(token_stream) => token_stream,
                None => return Ok::<_, anyhow::Error>(String::new()),
            };

        debug_component(
            debug_output,
            "channel",
            "begin_stream",
            Some((channel_id, user_id)),
        );
        if let Err(error) =
            cancellable_stage(turn, "stream_begin_delivery", channel.begin_stream(target)).await
        {
            tracing::warn!(channel_id, error = %error, "stream begin delivery failed");
            tracing::debug!(channel_id, error = ?error, "stream begin delivery failure details");
        }

        let mut full_text = String::new();
        let mut sentence_buffer = String::new();
        let mut sentence_tx = Some(sentence_tx);
        let mut stream_error: Option<anyhow::Error> = None;

        loop {
            let chunk_result = if let Some(turn) = turn {
                tokio::select! {
                biased;
                    _ = turn.token.cancelled() => {
                        turn.mark_interrupted_stage("llm_stream");
                        break;
                    }
                    chunk_result = token_stream.next() => chunk_result,
                }
            } else {
                token_stream.next().await
            };

            let Some(chunk_result) = chunk_result else {
                break;
            };
            match chunk_result {
                Ok(chunk) => {
                    full_text.push_str(&chunk);
                    sentence_buffer.push_str(&chunk);
                    debug_component_stream_chunk(debug_output, &chunk, Some((channel_id, user_id)));

                    if let Err(e) = cancellable_stage(
                        turn,
                        "stream_chunk_delivery",
                        channel.send_stream_chunk(target, &chunk),
                    )
                    .await
                    {
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
                    stream_error = Some(e);
                    break;
                }
            }
        }

        if let Some(error) = stream_error {
            if full_text.trim().is_empty() {
                drop(sentence_tx);
                let _ = cancellable_stage(
                    turn,
                    "stream_end_delivery",
                    channel.end_stream(target, &full_text),
                )
                .await;
                return Err(error);
            }
            let notice = "抱歉，后面的内容生成中断了。";
            full_text.push_str(notice);
            if let Err(e) = cancellable_stage(
                turn,
                "stream_error_notice",
                channel.send_stream_chunk(target, notice),
            )
            .await
            {
                tracing::warn!(error = %e, "stream error notice delivery failed");
                tracing::debug!(error = ?e, "stream error notice delivery failure details");
            }
            if let Some(tx) = sentence_tx.as_ref() {
                let _ = tx.send(notice.to_string()).await;
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

        if let Err(error) = cancellable_stage(
            turn,
            "stream_end_delivery",
            channel.end_stream(target, &full_text),
        )
        .await
        {
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

    let (llm_result, audio_result, tts_result) =
        futures::join!(llm_delivery, audio_delivery, tts_worker);
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
    if let Err(error) = tts_result {
        tracing::warn!(error = %error, "sentence-level TTS worker failed");
        tracing::debug!(error = ?error, "sentence-level TTS worker failure details");
    }

    Ok((llm_result?, audio_streamed))
}

#[derive(Debug, Clone, Deserialize)]
struct SentenceExpressionLlmBatch {
    #[serde(default)]
    items: Vec<SentenceExpressionLlmPatch>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SentenceExpressionLlmPatch {
    #[serde(default)]
    sentence_index: Option<usize>,
    #[serde(default)]
    dialogue_act: Option<String>,
    #[serde(default)]
    content_tone: Option<String>,
    #[serde(default)]
    style: Option<String>,
    #[serde(default)]
    intensity: Option<f32>,
    #[serde(default)]
    prosody_delta: Option<SentenceProsodyDelta>,
    #[serde(default)]
    confidence: f32,
    #[serde(default)]
    reason_code: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default)]
struct SentenceProsodyDelta {
    #[serde(default)]
    rate: f32,
    #[serde(default)]
    energy: f32,
    #[serde(default)]
    pitch_shift: f32,
    #[serde(default)]
    pause_bias: f32,
}

async fn resolve_sentence_speech(
    expression: Arc<crate::expression::ExpressionRuntime>,
    sentence_expression_llm: Option<Arc<dyn LlmAdapter>>,
    input: ExpressionInput,
    baseline: SpeechExpression,
    trace_context: Option<TtsTraceContext>,
    debug_output: bool,
    channel_id: &str,
    user_id: &str,
) -> SpeechExpression {
    let policy = expression.sentence_llm_policy();
    let Some(llm) = sentence_expression_llm else {
        debug_sentence_expression_decision(
            debug_output,
            channel_id,
            user_id,
            &input,
            &baseline,
            None,
            &baseline,
            "unavailable",
        );
        record_sentence_expression_trace(
            trace_context.as_ref(),
            &input,
            &baseline,
            None,
            &baseline,
            "unavailable",
        )
        .await;
        return baseline;
    };
    if !policy.enabled || policy.timeout_ms == 0 {
        debug_sentence_expression_decision(
            debug_output,
            channel_id,
            user_id,
            &input,
            &baseline,
            None,
            &baseline,
            "disabled",
        );
        record_sentence_expression_trace(
            trace_context.as_ref(),
            &input,
            &baseline,
            None,
            &baseline,
            "disabled",
        )
        .await;
        return baseline;
    }

    let cache_key = policy
        .cache_enabled
        .then(|| sentence_expression_cache_key(&input, &baseline));
    if let Some(key) = cache_key.as_ref()
        && let Some(cached) = sentence_expression_cache_get(key)
    {
        debug_sentence_expression_decision(
            debug_output,
            channel_id,
            user_id,
            &input,
            &baseline,
            None,
            &cached,
            "cache_hit",
        );
        record_sentence_expression_trace(
            trace_context.as_ref(),
            &input,
            &baseline,
            None,
            &cached,
            "cache_hit",
        )
        .await;
        return cached;
    }

    let Some(patch) = sentence_expression_llm_patch(llm.as_ref(), &policy, &input, &baseline).await
    else {
        debug_sentence_expression_decision(
            debug_output,
            channel_id,
            user_id,
            &input,
            &baseline,
            None,
            &baseline,
            "fallback",
        );
        record_sentence_expression_trace(
            trace_context.as_ref(),
            &input,
            &baseline,
            None,
            &baseline,
            "fallback",
        )
        .await;
        return baseline;
    };

    if patch.confidence < policy.min_confidence.clamp(0.0, 1.0) {
        debug_sentence_expression_decision(
            debug_output,
            channel_id,
            user_id,
            &input,
            &baseline,
            Some(&patch),
            &baseline,
            "rejected_low_confidence",
        );
        record_sentence_expression_trace(
            trace_context.as_ref(),
            &input,
            &baseline,
            Some(&patch),
            &baseline,
            "rejected_low_confidence",
        )
        .await;
        return baseline;
    }

    let mut patched_input = input.clone();
    if let Some(dialogue_act) = normalize_dialogue_act(patch.dialogue_act.as_deref()) {
        patched_input.dialogue_act = dialogue_act;
    }
    if let Some(content_tone) = normalize_content_tone(patch.content_tone.as_deref()) {
        patched_input.content_tone = content_tone;
    }
    if let Some(intensity) = patch.intensity {
        let capped = sentence_intensity_cap(&input, intensity);
        patched_input.constraints.max_intensity = Some(
            patched_input
                .constraints
                .max_intensity
                .unwrap_or(1.0)
                .min(capped),
        );
    }

    let mut final_speech = expression.speech_for_expression_input(patched_input).await;
    if let Some(style) = normalize_style(patch.style.as_deref())
        && style_override_allowed(&style, &input, &baseline, &patch)
    {
        final_speech.style = style;
    }
    if let Some(delta) = patch.prosody_delta {
        apply_sentence_prosody_delta(&mut final_speech, delta, &input);
    }

    debug_sentence_expression_decision(
        debug_output,
        channel_id,
        user_id,
        &input,
        &baseline,
        Some(&patch),
        &final_speech,
        "applied",
    );

    record_sentence_expression_trace(
        trace_context.as_ref(),
        &input,
        &baseline,
        Some(&patch),
        &final_speech,
        "applied",
    )
    .await;

    if let Some(key) = cache_key {
        sentence_expression_cache_put(key, final_speech.clone());
    }

    final_speech
}

fn sentence_expression_cache_get(key: &str) -> Option<SpeechExpression> {
    let cache = SENTENCE_EXPRESSION_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(key)
        .cloned()
}

fn sentence_expression_cache_put(key: String, speech: SpeechExpression) {
    let cache = SENTENCE_EXPRESSION_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cache.len() >= SENTENCE_EXPRESSION_CACHE_CAPACITY
        && let Some(first_key) = cache.keys().next().cloned()
    {
        cache.remove(&first_key);
    }
    cache.insert(key, speech);
}

fn sentence_expression_cache_key(input: &ExpressionInput, baseline: &SpeechExpression) -> String {
    serde_json::json!({
        "sentence": &input.response_text,
        "route": &input.route,
        "dialogue_act": &input.dialogue_act,
        "content_tone": &input.content_tone,
        "baseline_style": &baseline.style,
        "baseline_intensity": (baseline.intensity * 100.0).round() as i32,
        "active_task_serious": input.safety_context.active_task_serious,
        "minimal_expression": input.safety_context.minimal_expression,
        "night_mode": input.safety_context.night_mode,
        "allow_playful": input.constraints.allow_playful,
        "require_clarity": input.constraints.require_clarity,
        "prefer_minimal": input.constraints.prefer_minimal,
        "frustration": (input.user_state.frustration * 100.0).round() as i32,
    })
    .to_string()
}

fn debug_sentence_expression_decision(
    debug_output: bool,
    channel_id: &str,
    user_id: &str,
    input: &ExpressionInput,
    baseline: &SpeechExpression,
    patch: Option<&SentenceExpressionLlmPatch>,
    final_speech: &SpeechExpression,
    decision: &str,
) {
    debug_component_json(
        debug_output,
        "expression",
        "sentence_llm",
        &json!({
            "sentence": input.response_text,
            "route": input.route,
            "dialogue_act": input.dialogue_act,
            "content_tone": input.content_tone,
            "baseline": baseline,
            "patch": patch,
            "final": final_speech,
            "decision": decision,
        }),
        Some((channel_id, user_id)),
    );
}

async fn record_sentence_expression_trace(
    trace_context: Option<&TtsTraceContext>,
    input: &ExpressionInput,
    baseline: &SpeechExpression,
    patch: Option<&SentenceExpressionLlmPatch>,
    final_speech: &SpeechExpression,
    decision: &str,
) {
    let Some(trace_context) = trace_context else {
        return;
    };
    trace_context
        .record(
            "sentence_expression_llm",
            "expression",
            Some(format!("Sentence expression LLM {decision}")),
            json!({
                "sentence": input.response_text,
                "route": input.route,
                "baseline": baseline,
                "patch": patch,
                "final": final_speech,
                "decision": decision,
            }),
        )
        .await;
}

async fn sentence_expression_llm_patch(
    llm: &dyn LlmAdapter,
    policy: &SentenceExpressionLlmPolicySection,
    input: &ExpressionInput,
    baseline: &SpeechExpression,
) -> Option<SentenceExpressionLlmPatch> {
    let messages = sentence_expression_messages(input, baseline);
    let result = timeout(
        Duration::from_millis(policy.timeout_ms.max(1)),
        llm.chat(&messages),
    )
    .await;

    let raw = match result {
        Ok(Ok(raw)) => raw,
        Ok(Err(error)) => {
            tracing::debug!(error = ?error, "sentence expression LLM failed; using baseline");
            return None;
        }
        Err(_) => {
            tracing::debug!(
                timeout_ms = policy.timeout_ms,
                "sentence expression LLM timed out; using baseline"
            );
            return None;
        }
    };

    parse_sentence_expression_response(&raw)
}

fn sentence_expression_messages(
    input: &ExpressionInput,
    baseline: &SpeechExpression,
) -> Vec<ChatMessage> {
    let system = r#"You are a sentence-level expression reviewer for a voice assistant.
You do not rewrite or answer the user. Review one TTS sentence and optionally patch the baseline expression.
Return JSON only. No markdown fences, no explanation.

Allowed dialogue_act values: respond, explain, warn, apologize, comfort, celebrate, focus, decline.
Allowed content_tone values: neutral, technical, precise, serious, safety, supportive, playful, concise, minimal.
Allowed style values: neutral, gentle, calm, cheerful, serious.
Use low intensity for safety, apology, comfort, and technical explanation.
Never make serious, safety, apology, or comfort content cheerful.
Prefer calm or serious when the sentence contains caution, uncertainty, risk, or an instruction to stop/wait.
Use cheerful only for clearly positive, safe celebration.

Output schema:
{"items":[{"sentence_index":0,"dialogue_act":"explain","content_tone":"technical","style":"calm","intensity":0.32,"prosody_delta":{"rate":0.0,"energy":0.0,"pitch_shift":0.0,"pause_bias":0.0},"confidence":0.8,"reason_code":"short_code"}]}"#;

    let user = json!({
        "sentence_index": 0,
        "sentence": input.response_text,
        "route": input.route,
        "baseline": {
            "dialogue_act": input.dialogue_act,
            "content_tone": input.content_tone,
            "style": baseline.style,
            "intensity": baseline.intensity,
            "prosody": baseline.prosody,
        },
        "conversation": {
            "turn_kind": input.conversation_context.turn_kind,
            "used_tools": input.conversation_context.used_tools,
            "response_was_fallback": input.conversation_context.response_was_fallback,
            "cache_hit": input.conversation_context.cache_hit,
            "prior_message_count": input.conversation_context.prior_message_count,
        },
        "safety": {
            "active_task_serious": input.safety_context.active_task_serious,
            "minimal_expression": input.safety_context.minimal_expression,
            "night_mode": input.safety_context.night_mode,
            "volume_limit": input.safety_context.volume_limit,
            "allow_playful": input.constraints.allow_playful,
            "require_clarity": input.constraints.require_clarity,
            "prefer_minimal": input.constraints.prefer_minimal,
        },
        "user_state": {
            "frustration": input.user_state.frustration,
            "engagement": input.user_state.engagement,
        }
    })
    .to_string();

    vec![
        ChatMessage::new("system", system),
        ChatMessage::new("user", user),
    ]
}

fn parse_sentence_expression_response(raw: &str) -> Option<SentenceExpressionLlmPatch> {
    let json = strip_code_fences(raw);
    if let Ok(batch) = serde_json::from_str::<SentenceExpressionLlmBatch>(json)
        && let Some(item) = batch.items.into_iter().next()
    {
        return Some(item);
    }
    serde_json::from_str::<SentenceExpressionLlmPatch>(json).ok()
}

fn strip_code_fences(s: &str) -> &str {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("JSON"))
            .unwrap_or(rest);
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    }
}

fn normalize_dialogue_act(value: Option<&str>) -> Option<String> {
    normalize_enum(
        value,
        &[
            "respond",
            "explain",
            "warn",
            "apologize",
            "comfort",
            "celebrate",
            "focus",
            "decline",
        ],
    )
}

fn normalize_content_tone(value: Option<&str>) -> Option<String> {
    normalize_enum(
        value,
        &[
            "neutral",
            "technical",
            "precise",
            "serious",
            "safety",
            "supportive",
            "playful",
            "concise",
            "minimal",
        ],
    )
}

fn normalize_style(value: Option<&str>) -> Option<String> {
    normalize_enum(value, &["neutral", "gentle", "calm", "cheerful", "serious"])
}

fn normalize_enum(value: Option<&str>, allowed: &[&str]) -> Option<String> {
    let value = value?.trim().to_ascii_lowercase();
    allowed
        .iter()
        .any(|allowed| *allowed == value)
        .then_some(value)
}

fn sentence_intensity_cap(input: &ExpressionInput, requested: f32) -> f32 {
    let mut cap = requested.clamp(0.0, 1.0);
    if let Some(max) = input.safety_context.max_expression_intensity {
        cap = cap.min(max.clamp(0.0, 1.0));
    }
    if input.safety_context.night_mode || input.safety_context.minimal_expression {
        cap = cap.min(0.25);
    }
    if input.safety_context.active_task_serious {
        cap = cap.min(0.40);
    }
    cap
}

fn style_override_allowed(
    style: &str,
    input: &ExpressionInput,
    baseline: &SpeechExpression,
    patch: &SentenceExpressionLlmPatch,
) -> bool {
    if input.safety_context.night_mode || input.safety_context.minimal_expression {
        return matches!(style, "neutral" | "gentle" | "calm");
    }
    if input.safety_context.active_task_serious {
        return matches!(style, "gentle" | "calm" | "serious");
    }
    if style == "cheerful" {
        return input.constraints.allow_playful
            && input.user_state.frustration < 0.35
            && (baseline.style == "cheerful" || patch.confidence >= 0.85);
    }
    true
}

fn apply_sentence_prosody_delta(
    speech: &mut SpeechExpression,
    delta: SentenceProsodyDelta,
    input: &ExpressionInput,
) {
    let rate_delta = delta.rate.clamp(-0.12, 0.12);
    let energy_delta = delta.energy.clamp(-0.15, 0.15);
    let pitch_delta = delta.pitch_shift.clamp(-0.12, 0.12);
    let pause_delta = delta.pause_bias.clamp(-0.18, 0.18);

    speech.prosody.rate = (speech.prosody.rate + rate_delta).clamp(0.5, 2.0);
    speech.prosody.energy = (speech.prosody.energy + energy_delta).clamp(0.0, 1.0);
    speech.prosody.pitch_shift = (speech.prosody.pitch_shift + pitch_delta).clamp(-1.0, 1.0);
    speech.prosody.pause_bias = (speech.prosody.pause_bias + pause_delta).clamp(-1.0, 1.0);

    if let Some(volume_limit) = input.safety_context.volume_limit {
        speech.prosody.energy = speech.prosody.energy.min(volume_limit.clamp(0.0, 1.0));
    }
    if input.safety_context.night_mode || input.safety_context.minimal_expression {
        speech.prosody.energy = speech.prosody.energy.min(0.35);
        speech.prosody.rate = speech.prosody.rate.min(1.0);
        speech.prosody.pause_bias = speech.prosody.pause_bias.max(0.08);
    }

    if let Some(capabilities) = &input.tts_capabilities {
        speech.prosody.rate = speech.prosody.rate.clamp(
            capabilities.prosody_limits.min_rate,
            capabilities.prosody_limits.max_rate,
        );
        speech.prosody.energy = speech.prosody.energy.clamp(
            capabilities.prosody_limits.min_energy,
            capabilities.prosody_limits.max_energy,
        );
        speech.prosody.pitch_shift = speech.prosody.pitch_shift.clamp(
            capabilities.prosody_limits.min_pitch_shift,
            capabilities.prosody_limits.max_pitch_shift,
        );
        speech.prosody.pause_bias = speech.prosody.pause_bias.clamp(
            capabilities.prosody_limits.min_pause_bias,
            capabilities.prosody_limits.max_pause_bias,
        );
    }
}

async fn stream_tts_sentences(
    tts: Arc<dyn TtsAdapter>,
    expression: Arc<crate::expression::ExpressionRuntime>,
    sentence_expression_llm: Option<Arc<dyn LlmAdapter>>,
    mut sentence_rx: mpsc::Receiver<String>,
    audio_tx: mpsc::Sender<anyhow::Result<AudioPayload>>,
    audio_started: Arc<AtomicBool>,
    speech_context: Option<SentenceSpeechContext>,
    trace_context: Option<TtsTraceContext>,
    debug_output: bool,
    channel_id: String,
    user_id: String,
    cancel_token: Option<CancellationToken>,
) -> anyhow::Result<()> {
    loop {
        let sentence = if let Some(cancel_token) = cancel_token.as_ref() {
            tokio::select! {
                biased;
                _ = cancel_token.cancelled() => return Ok(()),
                sentence = sentence_rx.recv() => sentence,
            }
        } else {
            sentence_rx.recv().await
        };
        let Some(sentence) = sentence else {
            break;
        };
        let sentence = sentence.trim();
        if sentence.is_empty() {
            continue;
        }

        debug_component_text(
            debug_output,
            "tts",
            "sentence_call",
            sentence,
            Some((&channel_id, &user_id)),
        );
        let expression_input = speech_context
            .as_ref()
            .map(|context| context.input_for_sentence(sentence))
            .unwrap_or_else(|| ExpressionInput {
                response_text: sentence.to_string(),
                route: None,
                channel_id: channel_id.clone(),
                user_id: user_id.clone(),
                dialogue_act: "respond".to_string(),
                content_tone: "neutral".to_string(),
                user_state: Default::default(),
                device_context: Default::default(),
                safety_context: Default::default(),
                conversation_context: ExpressionConversationContext {
                    turn_kind: "streaming_sentence_tts".to_string(),
                    ..Default::default()
                },
                constraints: Default::default(),
                memory_context: None,
                voice_output: true,
                hardware_control: false,
                tts_capabilities: None,
                expression_capabilities: None,
            });
        let baseline_speech = expression
            .speech_for_sentence(expression_input.clone())
            .await;
        let speech = resolve_sentence_speech(
            expression.clone(),
            sentence_expression_llm.clone(),
            expression_input,
            baseline_speech,
            trace_context.clone(),
            debug_output,
            &channel_id,
            &user_id,
        )
        .await;
        let request = TtsRequest::with_speech(sentence, &speech);
        let adjustments = tts.provider_adjustments(&request).await;
        if !adjustments.is_empty() {
            debug_component_text(
                debug_output,
                "tts",
                "provider_adjustments",
                &serde_json::to_string(&adjustments).unwrap_or_else(|_| "[]".to_string()),
                Some((&channel_id, &user_id)),
            );
        }
        let synth = tts.synthesize_request(&request);
        let audio_bytes = if let Some(cancel_token) = cancel_token.as_ref() {
            tokio::select! {
                biased;
                _ = cancel_token.cancelled() => return Ok(()),
                result = synth => match result {
                    Ok(audio_bytes) => audio_bytes,
                    Err(error) => {
                        let message = format!("TTS sentence synthesis failed: {error}");
                        let _ = audio_tx.send(Err(anyhow::anyhow!(message))).await;
                        return Err(error);
                    }
                },
            }
        } else {
            match synth.await {
                Ok(audio_bytes) => audio_bytes,
                Err(error) => {
                    let message = format!("TTS sentence synthesis failed: {error}");
                    let _ = audio_tx.send(Err(anyhow::anyhow!(message))).await;
                    return Err(error);
                }
            }
        };

        let payload = match tts_bytes_to_pcm_payload(audio_bytes) {
            Ok(payload) => payload,
            Err(error) => {
                let message = format!("TTS sentence audio decode failed: {error}");
                let _ = audio_tx.send(Err(anyhow::anyhow!(message))).await;
                return Err(error);
            }
        };

        if payload.data.is_empty() {
            continue;
        }

        let silence =
            pcm16_silence_payload(payload.sample_rate, payload.channels, TTS_SENTENCE_GAP_MS);
        if audio_tx.send(Ok(payload)).await.is_err() {
            return Ok(());
        }
        audio_started.store(true, Ordering::Relaxed);
        if audio_tx.send(Ok(silence)).await.is_err() {
            return Ok(());
        }
    }

    Ok(())
}

pub fn tts_bytes_to_pcm_payload(bytes: Vec<u8>) -> anyhow::Result<AudioPayload> {
    if let Some((data, sample_rate, channels)) = pcm16_payload_from_wav(&bytes)? {
        return Ok(AudioPayload {
            data,
            sample_rate,
            channels,
            format: AudioFormat::Pcm16,
        });
    }

    // The TTS adapter contract historically returned WAV bytes, but keep a
    // conservative raw-PCM fallback for test/noop/custom adapters.
    Ok(AudioPayload {
        data: bytes,
        sample_rate: 24000,
        channels: 1,
        format: AudioFormat::Pcm16,
    })
}

fn pcm16_payload_from_wav(bytes: &[u8]) -> anyhow::Result<Option<(Vec<u8>, u32, u16)>> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Ok(None);
    }

    let mut pos = 12usize;
    let mut channels = None;
    let mut sample_rate = None;
    let mut bits_per_sample = None;
    let mut audio_format = None;
    let mut data_range = None;

    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into()?) as usize;
        let data_start = pos + 8;
        let data_end = data_start.saturating_add(size);
        if data_end > bytes.len() {
            break;
        }

        match chunk_id {
            b"fmt " if size >= 16 => {
                audio_format = Some(u16::from_le_bytes(
                    bytes[data_start..data_start + 2].try_into()?,
                ));
                channels = Some(u16::from_le_bytes(
                    bytes[data_start + 2..data_start + 4].try_into()?,
                ));
                sample_rate = Some(u32::from_le_bytes(
                    bytes[data_start + 4..data_start + 8].try_into()?,
                ));
                bits_per_sample = Some(u16::from_le_bytes(
                    bytes[data_start + 14..data_start + 16].try_into()?,
                ));
            }
            b"data" => data_range = Some((data_start, data_end)),
            _ => {}
        }

        let padded = size + (size % 2);
        pos = data_start.saturating_add(padded);
    }

    let Some((data_start, data_end)) = data_range else {
        anyhow::bail!("WAV missing data chunk");
    };
    let channels = channels.unwrap_or(1);
    let sample_rate = sample_rate.unwrap_or(24_000);
    let bits_per_sample = bits_per_sample.unwrap_or(16);
    let audio_format = audio_format.unwrap_or(1);

    if audio_format != 1 || bits_per_sample != 16 {
        anyhow::bail!(
            "unsupported WAV format: audio_format={}, bits_per_sample={}",
            audio_format,
            bits_per_sample
        );
    }

    let mut data = bytes[data_start..data_end].to_vec();
    if data.len() % 2 != 0 {
        data.pop();
    }
    Ok(Some((data, sample_rate, channels.max(1))))
}

pub fn pcm16_silence_payload(sample_rate: u32, channels: u16, duration_ms: u32) -> AudioPayload {
    let sample_count = sample_rate
        .saturating_mul(u32::from(channels.max(1)))
        .saturating_mul(duration_ms)
        .saturating_div(1_000) as usize;
    AudioPayload {
        data: vec![0u8; sample_count * 2],
        sample_rate,
        channels: channels.max(1),
        format: AudioFormat::Pcm16,
    }
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

pub(super) async fn stream_llm_response(
    _modules: &ModuleRegistry,
    llm: &dyn LlmAdapter,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    event_bus: &EventBus,
    messages: &[ChatMessage],
    debug_output: bool,
    turn: Option<&VoiceTurn>,
) -> anyhow::Result<String> {
    let mut token_stream =
        match cancellable_stage(turn, "llm_stream_start", llm.chat_stream(messages)).await? {
            Some(token_stream) => token_stream,
            None => return Ok(String::new()),
        };

    debug_component(
        debug_output,
        "channel",
        "begin_stream",
        Some((channel_id, user_id)),
    );
    if let Err(error) =
        cancellable_stage(turn, "stream_begin_delivery", channel.begin_stream(target)).await
    {
        tracing::warn!(channel_id, error = %error, "stream begin delivery failed");
        tracing::debug!(channel_id, error = ?error, "stream begin delivery failure details");
    }

    let mut full_text = String::new();
    let mut stream_error: Option<anyhow::Error> = None;

    loop {
        let chunk_result = if let Some(turn) = turn {
            tokio::select! {
                biased;
                _ = turn.token.cancelled() => {
                    turn.mark_interrupted_stage("llm_stream");
                    break;
                }
                chunk_result = token_stream.next() => chunk_result,
            }
        } else {
            token_stream.next().await
        };

        let Some(chunk_result) = chunk_result else {
            break;
        };
        match chunk_result {
            Ok(chunk) => {
                full_text.push_str(&chunk);
                debug_component_stream_chunk(debug_output, &chunk, Some((channel_id, user_id)));

                if let Err(e) = cancellable_stage(
                    turn,
                    "stream_chunk_delivery",
                    channel.send_stream_chunk(target, &chunk),
                )
                .await
                {
                    tracing::warn!(error = %e, "stream chunk delivery failed");
                    tracing::debug!(error = ?e, "stream chunk delivery failure details");
                }

                event_bus.publish(ServerEvent::StreamChunk {
                    channel_id: channel_id.to_string(),
                    user_id: user_id.to_string(),
                    chunk,
                });
            }
            Err(e) => {
                tracing::error!(error = %e, "LLM stream error");
                tracing::debug!(error = ?e, "LLM stream error details");
                stream_error = Some(e);
                break;
            }
        }
    }

    if let Some(error) = stream_error {
        if full_text.trim().is_empty() {
            let _ = cancellable_stage(
                turn,
                "stream_end_delivery",
                channel.end_stream(target, &full_text),
            )
            .await;
            return Err(error);
        }
        let notice = "抱歉，后面的内容生成中断了。";
        full_text.push_str(notice);
        if let Err(e) = cancellable_stage(
            turn,
            "stream_error_notice",
            channel.send_stream_chunk(target, notice),
        )
        .await
        {
            tracing::warn!(error = %e, "stream error notice delivery failed");
            tracing::debug!(error = ?e, "stream error notice delivery failure details");
        }
    }

    if let Err(error) = cancellable_stage(
        turn,
        "stream_end_delivery",
        channel.end_stream(target, &full_text),
    )
    .await
    {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::ProsodyControls;

    fn test_input() -> ExpressionInput {
        ExpressionInput {
            response_text: "好，问题找到了，但这里先别动。".to_string(),
            route: Some("major_llm".to_string()),
            channel_id: "test-channel".to_string(),
            user_id: "test-user".to_string(),
            dialogue_act: "respond".to_string(),
            content_tone: "neutral".to_string(),
            user_state: Default::default(),
            device_context: Default::default(),
            safety_context: Default::default(),
            conversation_context: Default::default(),
            constraints: ExpressionConstraints {
                allow_playful: true,
                ..Default::default()
            },
            memory_context: None,
            voice_output: true,
            hardware_control: false,
            tts_capabilities: None,
            expression_capabilities: None,
        }
    }

    fn test_speech(style: &str) -> SpeechExpression {
        SpeechExpression {
            style: style.to_string(),
            intensity: 0.5,
            voice_profile: None,
            prosody: ProsodyControls {
                rate: 1.0,
                energy: 0.5,
                pitch_shift: 0.0,
                pause_bias: 0.0,
            },
        }
    }

    #[test]
    fn parses_sentence_expression_patch_from_fenced_batch() {
        let patch = parse_sentence_expression_response(
            r#"```json
{"items":[{"sentence_index":0,"dialogue_act":"warn","content_tone":"safety","style":"serious","intensity":0.32,"prosody_delta":{"rate":-0.03,"energy":-0.05,"pitch_shift":0.0,"pause_bias":0.1},"confidence":0.86,"reason_code":"caution_after_progress"}]}
```"#,
        )
        .unwrap();

        assert_eq!(
            normalize_dialogue_act(patch.dialogue_act.as_deref()).unwrap(),
            "warn"
        );
        assert_eq!(normalize_style(patch.style.as_deref()).unwrap(), "serious");
        assert_eq!(patch.prosody_delta.unwrap().pause_bias, 0.1);
        assert!(patch.confidence > 0.8);
    }

    #[test]
    fn cheerful_override_needs_high_confidence_when_baseline_is_serious() {
        let input = test_input();
        let baseline = test_speech("serious");
        let patch = SentenceExpressionLlmPatch {
            sentence_index: Some(0),
            dialogue_act: Some("celebrate".to_string()),
            content_tone: Some("playful".to_string()),
            style: Some("cheerful".to_string()),
            intensity: Some(0.5),
            prosody_delta: None,
            confidence: 0.7,
            reason_code: None,
        };

        assert!(!style_override_allowed(
            "cheerful", &input, &baseline, &patch
        ));
    }

    #[test]
    fn prosody_delta_respects_volume_and_night_caps() {
        let mut input = test_input();
        input.safety_context.volume_limit = Some(0.4);
        input.safety_context.night_mode = true;
        let mut speech = test_speech("calm");

        apply_sentence_prosody_delta(
            &mut speech,
            SentenceProsodyDelta {
                rate: 0.2,
                energy: 0.5,
                pitch_shift: 0.2,
                pause_bias: -0.5,
            },
            &input,
        );

        assert!(speech.prosody.rate <= 1.0);
        assert!(speech.prosody.energy <= 0.35);
        assert!(speech.prosody.pause_bias >= 0.08);
    }
}
