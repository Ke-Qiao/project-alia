use crate::channels::adapter::ChannelAdapter;
use crate::{
    modules::interaction::routing::RouteDecision,
    providers::adapter::{ChatMessage, ToolCall, ToolDefinition, ToolResult},
};
use serde_json::Value;

pub(super) async fn log_channel_delivery_failure(
    channel: &dyn ChannelAdapter,
    error: anyhow::Error,
) {
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

pub(super) fn route_decision_name(decision: &RouteDecision) -> &'static str {
    match decision {
        RouteDecision::Skill { .. } => "skill",
        RouteDecision::MinorLlm { .. } => "minor_llm",
        RouteDecision::MajorLlm { .. } => "major_llm",
    }
}

pub(super) fn debug_component(
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

pub(super) fn debug_component_text(
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

pub(super) fn debug_component_json(
    enabled: bool,
    component: &'static str,
    event: &'static str,
    payload: &Value,
    channel: Option<(&str, &str)>,
) {
    if !enabled {
        tracing::debug!(
            target: "alia_server::debug_output",
            component,
            event,
            payload = %payload,
            "component json"
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
            payload = %payload,
            "component json"
        ),
        None => tracing::info!(
            target: "alia_server::debug_output",
            component,
            event,
            payload = %payload,
            "component json"
        ),
    }
}

pub(super) fn debug_component_audio(
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

pub(super) fn debug_component_route(
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

pub(super) fn debug_component_llm_call(
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

pub(super) fn debug_component_tool_selection(
    enabled: bool,
    available_tools: &[ToolDefinition],
    selected_tools: &[ToolDefinition],
    preferred_tool_names: &[String],
    channel: Option<(&str, &str)>,
) {
    let available_tools = available_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let selected_tools = selected_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let preferred_tools = preferred_tool_names.join(",");

    if !enabled {
        tracing::debug!(
            target: "alia_server::debug_output",
            component = "tools",
            event = "selection",
            available_tools,
            selected_tools,
            preferred_tools,
            "component tool selection"
        );
        return;
    }

    match channel {
        Some((channel_id, user_id)) => tracing::info!(
            target: "alia_server::debug_output",
            component = "tools",
            event = "selection",
            channel_id,
            user_id,
            available_tools,
            selected_tools,
            preferred_tools,
            "component tool selection"
        ),
        None => tracing::info!(
            target: "alia_server::debug_output",
            component = "tools",
            event = "selection",
            available_tools,
            selected_tools,
            preferred_tools,
            "component tool selection"
        ),
    }
}

pub(super) fn debug_component_tool_calls(
    enabled: bool,
    depth: u32,
    calls: &[ToolCall],
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

pub(super) fn debug_component_tool_result(
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

pub(super) fn debug_component_stream_chunk(
    enabled: bool,
    chunk: &str,
    channel: Option<(&str, &str)>,
) {
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
