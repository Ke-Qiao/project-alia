use super::context::validate_generation_context;
use super::debug::{
    debug_component_llm_call, debug_component_text, debug_component_tool_selection,
};
use super::delivery;
use super::services::InteractionServices;
use super::streaming::{
    SentenceSpeechContext, TtsTraceContext, stream_llm_response,
    stream_llm_response_with_sentence_tts,
};
use super::tools::{ToolSelection, run_llm_tool_loop};
use super::trace::AgentTrace;
use super::turn::{VoiceTurn, cancellable_stage, turn_is_interrupted};
use crate::channels::adapter::ChannelAdapter;
use crate::config::ToolsSection;
use crate::event::EventBus;
use crate::modules::delivery::transport::DeliveryTarget;
use crate::providers::adapter::{ChatMessage, ToolDefinition};
use crate::providers::llm::LlmService;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub(super) struct LlmExecutionOutcome {
    pub(super) text: String,
    pub(super) streamed: bool,
    pub(super) audio_streamed: bool,
    pub(super) used_tools: bool,
}

pub(super) fn allow_streaming_for_tools(
    tools: &[ToolDefinition],
    allow_streaming: bool,
    channel: &dyn ChannelAdapter,
) -> bool {
    allow_streaming && tools.is_empty() && channel.capabilities().streaming
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_llm_response(
    modules: &InteractionServices,
    llm: &LlmService,
    messages: Vec<ChatMessage>,
    tool_selection: ToolSelection,
    user_uuid: Uuid,
    session_id: Uuid,
    channel: &dyn ChannelAdapter,
    target: &DeliveryTarget,
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
    sentence_speech_context: Option<SentenceSpeechContext>,
) -> anyhow::Result<LlmExecutionOutcome> {
    validate_generation_context(modules, turn).await?;
    let available_tool_names = tool_selection
        .available_tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();
    let selected_tool_names = tool_selection
        .selected_tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();

    if !tool_selection.available_tools.is_empty()
        || !tool_selection.selected_tools.is_empty()
        || !tool_selection.preferred_tool_names.is_empty()
    {
        debug_component_tool_selection(
            debug_output,
            &tool_selection.available_tools,
            &tool_selection.selected_tools,
            &tool_selection.preferred_tool_names,
            Some((channel_id, user_id)),
        );
        trace
            .record(
                modules,
                "tool_selection",
                "tools",
                Some(route.to_string()),
                serde_json::json!({
                    "route": route,
                    "available_tools": available_tool_names.clone(),
                    "selected_tools": selected_tool_names.clone(),
                    "preferred_tools": tool_selection.preferred_tool_names.clone(),
                }),
            )
            .await;
    }

    if !tool_selection.selected_tools.is_empty()
        && llm.llm_capabilities().await.supports_tool_calling
    {
        trace
            .record(
                modules,
                "llm_call",
                "llm",
                Some(route.to_string()),
                serde_json::json!({
                    "route": route,
                    "mode": "tool_loop",
                    "selected_tools": selected_tool_names.clone(),
                    "messages": messages.len(),
                }),
            )
            .await;
        // The multi-round execution/context state is large. Keep it on the
        // heap instead of embedding it in every buffered/voice route future.
        let outcome = Box::pin(run_llm_tool_loop(
            modules,
            llm,
            messages,
            tool_selection.selected_tools,
            user_uuid,
            Some(session_id),
            channel_id,
            user_id,
            route,
            tools_config,
            debug_output,
            trace,
            turn,
        ))
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
        delivery::prepare_response(
            modules,
            turn,
            channel_id,
            user_id,
            Some(session_id),
            "assistant_response",
        )
        .await?;
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
        let (full, audio_streamed) = cancellable_stage(turn, "response_stream", async {
            Ok(if audio_response {
                stream_llm_response_with_sentence_tts(
                    modules,
                    llm,
                    channel,
                    target,
                    channel_id,
                    user_id,
                    event_bus,
                    &messages,
                    sentence_speech_context,
                    Some(TtsTraceContext::from_agent_trace(modules, trace)),
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
            })
        })
        .await?
        .unwrap_or_default();
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
    let full = match cancellable_stage(turn, "llm", llm.text(&messages)).await? {
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
        "minor model requested escalation"
    } else {
        reason
    })
}
