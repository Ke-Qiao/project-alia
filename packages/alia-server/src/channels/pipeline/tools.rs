use std::collections::{HashMap, HashSet};
use std::time::Duration;

use uuid::Uuid;

use super::debug::{
    debug_component_llm_call, debug_component_tool_calls, debug_component_tool_result,
};
use super::turn::{VoiceTurn, cancellable_stage, turn_is_interrupted};
use crate::config::ToolsSection;
use crate::module::ModuleRegistry;
use crate::providers::adapter::{
    ChatMessage, LlmToolCall, ToolCall, ToolContext, ToolDefinition, ToolResult,
};

const TOOL_USE_SYSTEM_PROMPT: &str = r#"Tool use rules:
- When an appropriate tool is available, call the tool instead of describing imaginary results.
- Never put raw tool-call JSON, function names, or hidden arguments in the final user-facing answer.
- After tool results are provided, produce only a natural-language answer for the user.
- If a tool result says confirmation is required, ask the user to confirm before claiming the action was done.
- If native tool calling is unavailable, request tools by replying only with JSON:
  {"tool_calls":[{"name":"tool_name","arguments":{}}]}
"#;

pub(super) async fn run_llm_tool_loop(
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
    turn: Option<&VoiceTurn>,
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

        let mut response = match cancellable_stage(
            turn,
            "llm_tool_call",
            modules.llm.chat_with_tools(&messages, &tools),
        )
        .await?
        {
            Some(response) => response,
            None => return Ok(String::new()),
        };
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
            if turn_is_interrupted(turn) {
                tracing::info!(
                    channel_id,
                    user_id = user_id_text,
                    tool = %call.tool_name,
                    "discarding tool result from interrupted voice turn"
                );
                return Ok(String::new());
            }
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

    let final_response = match cancellable_stage(turn, "llm", modules.llm.chat(&messages)).await? {
        Some(final_response) => final_response,
        None => String::new(),
    };
    Ok(final_response)
}

pub(super) async fn select_tools_for_skill(
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

pub(super) fn parse_text_tool_calls(content: &str) -> anyhow::Result<Vec<LlmToolCall>> {
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
