use super::debug::debug_component_json;
use super::dialogue_policy::DialoguePolicyInput;
use super::services::InteractionServices;
use super::streaming::SentenceSpeechContext;
use super::trace::AgentTrace;
use super::turn::{VoiceTurn, cancellable_stage};
use super::{dialogue_policy, turn};
use crate::channels::adapter::ChannelAdapter;
use crate::expression::orchestrator::{
    ExpressionExecutionPhase, execute_expression_plan_phase_with_response,
    negotiate_expression_capabilities,
};
use crate::expression::{
    ExpressionDeviceContext, ExpressionInput, ExpressionPlan, ExpressionSafetyContext,
    ExpressionUserState,
};
use crate::modules::delivery::transport::DeliveryTarget;

pub(super) struct PlannedExpression {
    pub(super) plan: ExpressionPlan,
    pub(super) input: ExpressionInput,
}

/// Keep the turn lock through interpretation commit and projection creation.
/// Cancellation can lose an acknowledgement; it cannot replace the source.
pub(super) async fn project_turn_affect(
    modules: &InteractionServices,
    state: &mut turn::TurnContext,
) -> anyhow::Result<()> {
    if state.frozen {
        return Ok(());
    }
    let Some(original) = state.generation.as_ref().cloned() else {
        return Ok(());
    };
    let mut changed = original.affect().is_none() || state.affect_dirty;
    if original.policy().appraisal.affect.enabled {
        if !state.user_appraised {
            if let Err(error) = Box::pin(
                modules
                    .expression
                    .observe_context_user(&modules.context, &original),
            )
            .await
            {
                if !error
                    .downcast_ref::<crate::modules::memory::context::ContextError>()
                    .is_some_and(|e| {
                        matches!(e, crate::modules::memory::context::ContextError::Revoked(_))
                    })
                {
                    return Err(error);
                }
            }
            state.user_appraised = true;
            changed = true;
        }
        let recalls = original.all_materialized_memory().recall_events;
        let digest = crate::modules::memory::context::evidence_digest(&serde_json::json!(recalls))?;
        if !recalls.is_empty() && state.recall_digest.as_ref() != Some(&digest) {
            if let Err(error) = Box::pin(
                modules
                    .expression
                    .observe_context_recall(&modules.context, &original),
            )
            .await
            {
                if !error
                    .downcast_ref::<crate::modules::memory::context::ContextError>()
                    .is_some_and(|e| {
                        matches!(e, crate::modules::memory::context::ContextError::Revoked(_))
                    })
                {
                    return Err(error);
                }
            }
            state.recall_digest = Some(digest);
            changed = true;
        }
    }
    if changed {
        state.generation = Some(Box::pin(modules.context.extend_affect(&original)).await?);
        state.affect_dirty = false;
    }
    Ok(())
}

pub(super) async fn observe_user_affect_for_turn(
    modules: &InteractionServices,
    turn: &VoiceTurn,
) -> anyhow::Result<()> {
    let mut state = turn.context.lock().await;
    Box::pin(project_turn_affect(modules, &mut state)).await
}

pub(super) async fn plan_expression_for_response(
    modules: &InteractionServices,
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
    generation: crate::modules::memory::context::GenerationContext,
    debug_output: bool,
) -> anyhow::Result<PlannedExpression> {
    let capabilities = channel.capabilities();
    let expression_capabilities = if capabilities.hardware_control {
        Some(negotiate_expression_capabilities(channel).await)
    } else {
        None
    };
    let memory_context = Some(generation.all_materialized_memory());
    let affect = generation
        .affect()
        .ok_or_else(|| anyhow::anyhow!("expression has no frozen affect projection"))?
        .snapshot();
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
    let plan = Box::pin(modules.expression.plan_with_context(
        &modules.context,
        &generation,
        expression_input.clone(),
    ))
    .await?;
    let _ = (trace, debug_output);
    tracing::debug!(plan_id=%plan.id,context_id=%generation.lease().id(),"expression planned from frozen context");
    Ok(PlannedExpression {
        plan,
        input: expression_input,
    })
}

pub(super) fn expression_user_state_from_affect(
    affect: &crate::expression::AffectSnapshot,
) -> ExpressionUserState {
    ExpressionUserState {
        frustration: ((-affect.affect.valence).max(0.0)
            + (affect.affect.arousal - 0.55).max(0.0) * 0.5)
            .clamp(0.0, 1.0),
        engagement: affect.social.engagement.clamp(0.0, 1.0),
    }
}

pub(super) fn expression_device_context(
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

pub(super) fn expression_safety_context(
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

pub(super) async fn execute_expression_and_trace(
    orchestrator: &crate::expression::orchestrator::ExpressionOrchestrator,
    channel: &dyn ChannelAdapter,
    target: &DeliveryTarget,
    plan: &ExpressionPlan,
    phase: Option<ExpressionExecutionPhase>,
    debug_output: bool,
    channel_id: &str,
    user_id: &str,
    turn: Option<&VoiceTurn>,
) {
    let execution = cancellable_stage(turn, "expression", async {
        Ok(execute_expression_plan_phase_with_response(
            orchestrator,
            channel,
            target,
            plan,
            phase.unwrap_or(ExpressionExecutionPhase::Full),
            turn.and_then(|turn| turn.response_id.get().copied()),
        )
        .await)
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

#[allow(clippy::too_many_arguments)]
pub(super) async fn build_sentence_speech_context(
    modules: &InteractionServices,
    turn: &VoiceTurn,
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
    enabled: bool,
) -> anyhow::Result<Option<SentenceSpeechContext>> {
    if !enabled || !channel.capabilities().voice_output {
        return Ok(None);
    }

    observe_user_affect_for_turn(modules, turn).await?;
    let generation = turn.generation_context().await?;
    let affect = generation
        .affect()
        .ok_or_else(|| anyhow::anyhow!("sentence context has no affect projection"))?
        .snapshot();
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
    Ok(Some(SentenceSpeechContext {
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
        memory_context: Some(generation.all_materialized_memory()),
        generation,
        tts_capabilities: Some(modules.tts.tts_capabilities().await),
    }))
}
