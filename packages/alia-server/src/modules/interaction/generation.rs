use super::cache::{
    CacheContext, CacheWrite, cache_write_for_response, cached_response_is_safe_direct,
    semantic_cache_candidates, semantic_cache_reference_prompt, semantic_candidate_is_safe_direct,
    semantic_candidate_is_safe_reference,
};
use super::context::{ensure_turn_context, merge_turn_cache, validate_generation_context};
use super::debug::{
    debug_component, debug_component_route, debug_component_text, route_decision_name,
};
use super::delivery;
use super::expression::{
    build_sentence_speech_context, observe_user_affect_for_turn, plan_expression_for_response,
};
use super::llm::{
    LlmExecutionOutcome, allow_streaming_for_tools, escalation_reason, execute_llm_response,
};
use super::persistence::{
    PendingTextPersistence, persist_interrupted_input_context, persist_text_interaction,
};
use super::prompt::{
    RuntimePromptContext, append_history_with_interrupted_context, build_base_system_messages,
    build_llm_messages, preview_for_prompt, runtime_context_has_user_memory,
    stt_transcript_context_prompt, system_message, tts_optimized_output_prompt,
};
use super::services::InteractionServices;
use super::streaming::TtsTraceContext;
use super::tools::{select_all_tools, select_tools_for_skill};
use super::trace::AgentTrace;
use super::turn::{VoiceTurn, cancellable_stage, turn_is_interrupted};
use crate::channels::adapter::ChannelAdapter;
use crate::config::{
    CachePolicyResourceSection, CacheSection, ConversationSection, MemorySection, ToolsSection,
};
use crate::event::EventBus;
use crate::expression::{ExpressionInput, ExpressionPlan};
use crate::modules::delivery::transport::DeliveryTarget;
use crate::{
    modules::interaction::{
        cache::DEFAULT_CACHE_SCOPE,
        routing::{RouteDecision, RouterInput},
    },
    providers::adapter::{ChatMessage, LlmAdapter},
};
use uuid::Uuid;

/// Result of `process_text` — carries the full response text and whether it
/// was already delivered to the channel via streaming.
pub(super) struct TextResult {
    pub(super) text: String,
    /// `true` when generation already ran through its output stream. A caller
    /// must not create another delivery or infer success from this flag.
    pub(super) streamed: bool,
    /// `true` when voice audio was already streamed while text was generated.
    /// Callers must NOT synthesize and send the same answer again.
    pub(super) audio_streamed: bool,
    /// `true` when processing stopped because the active voice turn was interrupted.
    pub(super) interrupted: bool,
    /// Persist this interaction after downstream delivery if the caller deferred
    /// history/cache writes to avoid storing interrupted assistant output.
    pub(super) pending_persistence: Option<PendingTextPersistence>,
    pub(super) expression_plan: Option<ExpressionPlan>,
    pub(super) expression_input: Option<ExpressionInput>,
    pub(super) tts_trace: Option<TtsTraceContext>,
}

impl TextResult {
    pub(super) fn interrupted() -> Self {
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

pub(super) fn process_text<'a>(
    text: &'a str,
    user_id: &'a str,
    modules: &'a InteractionServices,
    channel: &'a dyn ChannelAdapter,
    target: &'a DeliveryTarget,
    channel_id: &'a str,
    event_bus: &'a EventBus,
    debug_output: bool,
    audio_response: bool,
    input_from_stt: bool,
    conversation_config: &'a ConversationSection,
    tools_config: &'a ToolsSection,
    cache_config: &'a CacheSection,
    cache_policy: &'a CachePolicyResourceSection,
    memory_config: &'a MemorySection,
    turn: Option<&'a VoiceTurn>,
    defer_persistence: bool,
) -> futures::future::BoxFuture<'a, anyhow::Result<TextResult>> {
    // Keep the generation state machine on the heap across all text and voice
    // callers; nesting it in ingress futures can overflow default worker stacks.
    Box::pin(async move {
        let request = GenerationRequest {
            text,
            user_id,
            modules,
            channel,
            target,
            channel_id,
            event_bus,
            debug_output,
            audio_response,
            input_from_stt,
            conversation_config,
            tools_config,
            cache_config,
            cache_policy,
            memory_config,
            turn,
            defer_persistence,
        };
        let Some(mut prepared) = prepare_turn(&request).await? else {
            return Ok(TextResult::interrupted());
        };
        let cached = lookup_cache(&request, &mut prepared).await?;
        if let Some(turn) = turn.filter(|turn| turn.token.is_cancelled()) {
            // A cancelled lookup has already admitted the user source. Preserve
            // its terminal audit here even though routing/output never begins.
            // A cancellation during the cache trace await may not have a stage.
            turn.mark_interrupted_stage("after_cache_lookup");
            persist_interrupted_input_context(
                modules,
                prepared.session_id,
                prepared.uid,
                user_id,
                text,
                channel_id,
                turn,
                debug_output,
            )
            .await;
            let interruption = turn.interruption();
            prepared
                .trace
                .record(
                    modules,
                    "turn_interrupted",
                    "pipeline",
                    Some("Turn interrupted during cache lookup".to_string()),
                    serde_json::json!({"stage": interruption.stage, "reason": interruption.reason}),
                )
                .await;
            return Ok(TextResult {
                tts_trace: Some(TtsTraceContext::from_agent_trace(modules, &prepared.trace)),
                ..TextResult::interrupted()
            });
        }
        if let Some(cached) = cached {
            return complete_answer(
                &request,
                &mut prepared,
                CompletedAnswer {
                    outcome: LlmExecutionOutcome {
                        text: cached.text,
                        streamed: false,
                        audio_streamed: false,
                        used_tools: false,
                    },
                    route: "cache",
                    cache_write: None,
                    cache_hit: true,
                    response_was_fallback: false,
                    cache_kind: Some(cached.kind),
                    similarity: cached.similarity,
                },
            )
            .await;
        }
        let Some(routed) = Box::pin(route_response(&request, &mut prepared)).await? else {
            return Ok(TextResult {
                tts_trace: Some(TtsTraceContext::from_agent_trace(modules, &prepared.trace)),
                ..TextResult::interrupted()
            });
        };
        finish_generation(&request, &mut prepared, routed).await
    })
}

#[derive(Clone, Copy)]
struct GenerationRequest<'a> {
    text: &'a str,
    user_id: &'a str,
    modules: &'a InteractionServices,
    channel: &'a dyn ChannelAdapter,
    target: &'a DeliveryTarget,
    channel_id: &'a str,
    event_bus: &'a EventBus,
    debug_output: bool,
    audio_response: bool,
    input_from_stt: bool,
    conversation_config: &'a ConversationSection,
    tools_config: &'a ToolsSection,
    cache_config: &'a CacheSection,
    cache_policy: &'a CachePolicyResourceSection,
    memory_config: &'a MemorySection,
    turn: Option<&'a VoiceTurn>,
    defer_persistence: bool,
}

struct PreparedTurn {
    uid: Uuid,
    session_id: Uuid,
    source_event_id: Uuid,
    source_received_at: chrono::DateTime<chrono::Utc>,
    source_privacy_epoch: i64,
    history: Vec<ChatMessage>,
    base_system_messages: Vec<ChatMessage>,
    runtime_context: RuntimePromptContext,
    memory_context_used: bool,
    effective_cache: CacheSection,
    effective_memory: MemorySection,
    trace: AgentTrace,
}

struct CachedAnswer {
    text: String,
    kind: &'static str,
    similarity: Option<f32>,
}

struct RoutedAnswer {
    outcome: LlmExecutionOutcome,
    skill_cacheable: Option<bool>,
    route: &'static str,
}

struct CompletedAnswer {
    outcome: LlmExecutionOutcome,
    route: &'static str,
    cache_write: Option<CacheWrite>,
    cache_hit: bool,
    response_was_fallback: bool,
    cache_kind: Option<&'static str>,
    similarity: Option<f32>,
}
async fn prepare_turn(request: &GenerationRequest<'_>) -> anyhow::Result<Option<PreparedTurn>> {
    let GenerationRequest {
        text,
        user_id,
        modules,
        channel_id,
        audio_response,
        input_from_stt,
        cache_config,
        memory_config,
        turn,
        ..
    } = *request;
    let admitted_turn = turn.ok_or_else(|| anyhow::anyhow!("input has no admitted turn"))?;
    if turn_is_interrupted(turn) {
        return Ok(None);
    }
    let generation = match cancellable_stage(
        turn,
        "materialize_context",
        ensure_turn_context(modules, admitted_turn, channel_id, user_id, text),
    )
    .await?
    {
        Some(context) => context,
        None => return Ok(None),
    };
    let uid = generation.source().owner_id;
    let session_id = generation
        .source()
        .conversation_id
        .ok_or_else(|| anyhow::anyhow!("generation has no conversation"))?;
    let source_event_id = generation
        .source()
        .source_event_id
        .ok_or_else(|| anyhow::anyhow!("generation has no original source"))?;
    let source_received_at = generation.source().source_received_at;
    let source_privacy_epoch = generation.source().source_privacy_epoch;
    let history = generation.history_messages();
    let (mut base_system_messages, runtime_context) = match cancellable_stage(
        turn,
        "context_prompt",
        build_base_system_messages(uid, modules, &generation),
    )
    .await?
    {
        Some(context) => context,
        None => return Ok(None),
    };
    if input_from_stt {
        base_system_messages.push(system_message(stt_transcript_context_prompt()));
    }
    if audio_response {
        base_system_messages.push(system_message(tts_optimized_output_prompt(text)));
    }
    let memory_context_used = runtime_context_has_user_memory(&runtime_context);
    let policy = generation.policy();
    let mut effective_cache = cache_config.clone();
    effective_cache.enabled = policy.cache_enabled;
    effective_cache.exact_match_enabled = policy.cache_exact_enabled;
    effective_cache.semantic_match_enabled = policy.cache_semantic_enabled;
    effective_cache.semantic_threshold = policy.cache_semantic_threshold;
    effective_cache.default_ttl_seconds = policy.cache_default_ttl_seconds;
    effective_cache.skill_cache_ttl_seconds = policy.cache_skill_ttl_seconds;
    let mut effective_memory = memory_config.clone();
    effective_memory.enabled = policy.memory_enabled;
    effective_memory.capture_mode = policy.capture_mode;
    effective_memory.sensitive_filter_enabled = policy.sensitive_filter_enabled;
    effective_memory.paused_until = policy.paused_until.map(|until| until.to_rfc3339());
    effective_memory.capture_min_confidence = policy.capture_min_confidence;
    effective_memory.capture_min_importance = policy.capture_min_importance;

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
    match cancellable_stage(turn, "observe_user_affect_for_turn", async {
        observe_user_affect_for_turn(modules, admitted_turn).await
    })
    .await?
    {
        Some(value) => value,
        None => return Ok(None),
    };

    Ok(Some(PreparedTurn {
        uid,
        session_id,
        source_event_id,
        source_received_at,
        source_privacy_epoch,
        history,
        base_system_messages,
        runtime_context,
        memory_context_used,
        effective_cache,
        effective_memory,
        trace,
    }))
}
async fn lookup_cache(
    request: &GenerationRequest<'_>,
    prepared: &mut PreparedTurn,
) -> anyhow::Result<Option<CachedAnswer>> {
    let GenerationRequest {
        text,
        user_id,
        modules,
        channel_id,
        debug_output,
        turn,
        ..
    } = *request;
    let admitted_turn = turn.ok_or_else(|| anyhow::anyhow!("input has no admitted turn"))?;
    let uid = prepared.uid;
    let base_system_messages = &mut prepared.base_system_messages;
    let memory_context_used = prepared.memory_context_used;
    let cache_config = &prepared.effective_cache;
    let trace = &mut prepared.trace;
    // 2. Check exact cache first (never streamed). Only entries with explicit
    // safe-direct metadata can bypass the LLM.
    if cache_config.enabled && cache_config.exact_match_enabled && !memory_context_used {
        if let Some(cached) = cancellable_stage(
            turn,
            "cache_lookup",
            modules.cache.get(uid, DEFAULT_CACHE_SCOPE, text),
        )
        .await?
        .flatten()
        {
            if cached_response_is_safe_direct(&cached) {
                merge_turn_cache(modules, admitted_turn, &cached).await?;
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
                return Ok(Some(CachedAnswer {
                    text: response_text,
                    kind: "exact",
                    similarity: None,
                }));
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
                None => return Ok(None),
            };
        if let Some(candidate) = semantic_candidates.iter().find(|candidate| {
            candidate.similarity >= cache_config.semantic_threshold
                && semantic_candidate_is_safe_direct(candidate)
        }) {
            merge_turn_cache(modules, admitted_turn, &candidate.cached).await?;
            let cache_metadata = candidate.cached.metadata.clone();
            let response_text = candidate.cached.response_text.clone();
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
            return Ok(Some(CachedAnswer {
                text: response_text,
                kind: "semantic",
                similarity: Some(similarity),
            }));
        }

        if let Some((prompt, candidate_count)) =
            semantic_cache_reference_prompt(&semantic_candidates, debug_output)
        {
            for candidate in semantic_candidates
                .iter()
                .filter(|candidate| semantic_candidate_is_safe_reference(candidate))
            {
                merge_turn_cache(modules, admitted_turn, &candidate.cached).await?;
            }
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

    Ok(None)
}
async fn route_response(
    request: &GenerationRequest<'_>,
    prepared: &mut PreparedTurn,
) -> anyhow::Result<Option<RoutedAnswer>> {
    let GenerationRequest {
        text,
        user_id,
        modules,
        channel_id,
        debug_output,
        conversation_config,
        turn,
        ..
    } = *request;
    let uid = prepared.uid;
    let session_id = prepared.session_id;
    let history = &prepared.history;
    let base_system_messages = &mut prepared.base_system_messages;
    let runtime_context = &prepared.runtime_context;
    let trace = &mut prepared.trace;
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
        conversation_context: history.to_vec(),
        assistant_name: runtime_context.assistant_name.clone(),
    };
    validate_generation_context(modules, turn).await?;
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
                return Ok(None);
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

    let routed = match decision {
        RouteDecision::Skill { skill_name } => {
            let skill_match = crate::modules::interaction::skills::SkillMatch {
                name: skill_name,
                confidence: 1.0,
            };
            let Some(resolution) =
                cancellable_stage(turn, "skill_context", modules.skills.resolve(&skill_match))
                    .await?
            else {
                return Ok(None);
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
            let mut messages = base_system_messages.clone();
            if let Some(prompt) = &resolution.system_prompt {
                messages.push(ChatMessage::new("system", prompt));
            }
            messages.push(ChatMessage::new("system", "The active skill provides domain context only. Preferred tools are ranking hints, not restrictions; use whichever available tool best matches the user's request."));
            append_history_with_interrupted_context(&mut messages, history);
            messages.push(ChatMessage {
                role: "user".into(),
                content: text.into(),
                name: Some(user_id.into()),
                ..ChatMessage::default()
            });
            let Some(outcome) = run_model(
                request,
                prepared,
                modules.llm.as_ref(),
                messages,
                Some(&resolution.preferred_tools),
                "skill",
                conversation_config.streaming_enabled,
            )
            .await?
            else {
                return Ok(None);
            };
            RoutedAnswer {
                outcome,
                skill_cacheable: Some(resolution.cacheable),
                route: "skill",
            }
        }
        RouteDecision::MinorLlm {
            system_prompt,
            escalation_prompt,
        } => {
            let messages =
                build_llm_messages(base_system_messages, history, system_prompt, text, user_id);
            let Some(minor) = run_model(
                request,
                prepared,
                modules.minor_llm.as_ref(),
                messages,
                None,
                "minor_llm",
                conversation_config.streaming_enabled && escalation_prompt.is_none(),
            )
            .await?
            else {
                return Ok(None);
            };
            if let Some(reason) = escalation_reason(&minor.text)
                && let Some(major_prompt) = escalation_prompt
            {
                tracing::info!(channel_id, user_id, reason = %reason, "minor LLM escalated turn to major LLM");
                debug_component_text(
                    debug_output,
                    "llm",
                    "escalate",
                    reason,
                    Some((channel_id, user_id)),
                );
                let mut messages = build_llm_messages(
                    &prepared.base_system_messages,
                    &prepared.history,
                    Some(major_prompt),
                    text,
                    user_id,
                );
                messages.push(ChatMessage::new("system", format!("The minor model escalated this request instead of answering. Escalation reason: {reason}. Answer the original user request directly.")));
                let Some(outcome) = run_model(
                    request,
                    prepared,
                    modules.major_llm.as_ref(),
                    messages,
                    None,
                    "major_llm_escalated",
                    conversation_config.streaming_enabled,
                )
                .await?
                else {
                    return Ok(None);
                };
                RoutedAnswer {
                    outcome,
                    skill_cacheable: None,
                    route: "major_llm_escalated",
                }
            } else {
                RoutedAnswer {
                    outcome: minor,
                    skill_cacheable: None,
                    route: "minor_llm",
                }
            }
        }
        RouteDecision::MajorLlm { system_prompt } => {
            let messages =
                build_llm_messages(base_system_messages, history, system_prompt, text, user_id);
            let Some(outcome) = run_model(
                request,
                prepared,
                modules.major_llm.as_ref(),
                messages,
                None,
                "major_llm",
                conversation_config.streaming_enabled,
            )
            .await?
            else {
                return Ok(None);
            };
            RoutedAnswer {
                outcome,
                skill_cacheable: None,
                route: "major_llm",
            }
        }
    };
    Ok(Some(routed))
}

/// Shared skill/minor/major execution keeps tool selection and output policy identical.
async fn run_model(
    request: &GenerationRequest<'_>,
    prepared: &mut PreparedTurn,
    llm: &dyn LlmAdapter,
    messages: Vec<ChatMessage>,
    preferred_tools: Option<&[String]>,
    route: &'static str,
    allow_streaming: bool,
) -> anyhow::Result<Option<LlmExecutionOutcome>> {
    let GenerationRequest {
        modules,
        text,
        channel,
        channel_id,
        target,
        user_id,
        event_bus,
        tools_config,
        debug_output,
        turn,
        audio_response,
        ..
    } = *request;
    let admitted_turn = turn.ok_or_else(|| anyhow::anyhow!("input has no admitted turn"))?;
    let selection = async {
        match preferred_tools {
            Some(preferred) => {
                select_tools_for_skill(
                    modules,
                    preferred,
                    text,
                    tools_config,
                    channel_id,
                    &prepared.runtime_context.assistant_name,
                )
                .await
            }
            None => {
                select_all_tools(
                    modules,
                    text,
                    tools_config,
                    channel_id,
                    &prepared.runtime_context.assistant_name,
                )
                .await
            }
        }
    };
    let Some(tool_selection) = cancellable_stage(
        turn,
        if preferred_tools.is_some() {
            "select_tools_for_skill"
        } else {
            "tool_selection"
        },
        selection,
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(sentence_context) = cancellable_stage(
        turn,
        "build_sentence_speech_context",
        build_sentence_speech_context(
            modules,
            admitted_turn,
            channel,
            channel_id,
            user_id,
            route,
            false,
            false,
            false,
            text,
            prepared.history.len(),
            prepared.memory_context_used,
            audio_response
                && allow_streaming_for_tools(
                    &tool_selection.selected_tools,
                    allow_streaming,
                    channel,
                ),
        ),
    )
    .await?
    else {
        return Ok(None);
    };
    execute_llm_response(
        modules,
        llm,
        messages,
        tool_selection,
        prepared.uid,
        prepared.session_id,
        channel,
        target,
        channel_id,
        user_id,
        event_bus,
        route,
        audio_response,
        allow_streaming,
        tools_config,
        debug_output,
        &mut prepared.trace,
        turn,
        sentence_context,
    )
    .await
    .map(Some)
}

async fn finish_generation(
    request: &GenerationRequest<'_>,
    prepared: &mut PreparedTurn,
    routed: RoutedAnswer,
) -> anyhow::Result<TextResult> {
    let GenerationRequest {
        text,
        user_id,
        modules,
        channel_id,
        debug_output,
        cache_policy,
        turn,
        ..
    } = *request;
    let uid = prepared.uid;
    let session_id = prepared.session_id;
    let memory_context_used = prepared.memory_context_used;
    let cache_config = &prepared.effective_cache;
    let trace = &mut prepared.trace;
    let RoutedAnswer {
        outcome,
        skill_cacheable,
        route: route_for_cache,
    } = routed;
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

    complete_answer(
        request,
        prepared,
        CompletedAnswer {
            outcome: LlmExecutionOutcome {
                text: response,
                streamed,
                audio_streamed,
                used_tools,
            },
            route: route_for_cache,
            cache_write,
            cache_hit: false,
            response_was_fallback,
            cache_kind: None,
            similarity: None,
        },
    )
    .await
}
async fn complete_answer(
    request: &GenerationRequest<'_>,
    prepared: &mut PreparedTurn,
    answer: CompletedAnswer,
) -> anyhow::Result<TextResult> {
    let GenerationRequest {
        text,
        user_id,
        modules,
        channel,
        channel_id,
        event_bus,
        debug_output,
        turn,
        defer_persistence,
        ..
    } = *request;
    let admitted_turn = turn.ok_or_else(|| anyhow::anyhow!("input has no admitted turn"))?;
    let uid = prepared.uid;
    let session_id = prepared.session_id;
    let source_event_id = prepared.source_event_id;
    let source_received_at = prepared.source_received_at;
    let source_privacy_epoch = prepared.source_privacy_epoch;
    let history = &prepared.history;
    let prior_message_count = history.len();
    let memory_context_used = prepared.memory_context_used;
    let memory_config = &prepared.effective_memory;
    let mut trace = &mut prepared.trace;
    let CompletedAnswer {
        outcome,
        route: route_for_cache,
        cache_write,
        cache_hit,
        response_was_fallback,
        cache_kind,
        similarity,
    } = answer;
    let LlmExecutionOutcome {
        text: response,
        streamed,
        audio_streamed,
        used_tools,
    } = outcome;
    let pending_persistence = PendingTextPersistence {
        source_received_at,
        source_privacy_epoch,
        source_event_id: Some(source_event_id),
        response_id: delivery::prepare_response(
            modules,
            turn,
            channel_id,
            user_id,
            Some(session_id),
            "assistant_response",
        )
        .await?,
        session_id,
        user_id: uid,
        user_id_text: user_id.to_string(),
        query_text: text.to_string(),
        response_text: response.clone(),
        channel_id: channel_id.to_string(),
        cache_write,
        context: admitted_turn.context_binding().await?,
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
    let mut completion_payload = serde_json::json!({
        "route": route_for_cache,
        "streamed": streamed,
        "audio_streamed": audio_streamed,
        "used_tools": used_tools,
        "response_chars": response.chars().count(),
    });
    let summary = match cache_kind {
        Some("semantic") => {
            completion_payload["cache_kind"] = "semantic".into();
            completion_payload["similarity"] = serde_json::json!(similarity);
            "Turn completed from semantic cache"
        }
        Some(_) => "Turn completed from cache",
        None => {
            completion_payload["defer_persistence"] = defer_persistence.into();
            "Turn completed"
        }
    };
    trace
        .record(
            modules,
            "turn_completed",
            "pipeline",
            Some(summary.to_string()),
            completion_payload,
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
            cache_hit,
            text,
            prior_message_count,
            memory_context_used,
            admitted_turn.generation_context().await?,
            debug_output,
        )
        .await?)
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

const EMPTY_RESPONSE_FALLBACK: &str = "抱歉，我刚才没有生成有效回复，请再试一次。";
