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
use crate::registry::is_plugin_source;

const TEXT_TOOL_CALL_SCHEMA: &str = "alia.tool_calls.v1";
const MAX_TEXT_TOOL_CALLS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolCallProtocol {
    Native,
    TextFallback,
}

const TOOL_USE_SYSTEM_PROMPT: &str = r#"Tool use rules:
- Skills are cognitive context only. Never try to call a skill by name.
- Tools are the only executable units. When an appropriate tool is available, call the tool instead of describing imaginary results.
- Use native tool/function calling whenever the provider supports it. Tool arguments must be valid JSON matching the tool's parameter schema.
- If native tool calling is unavailable and a tool is needed, reply with exactly one JSON object and no prose:
  {"schema":"alia.tool_calls.v1","tool_calls":[{"name":"tool_name","arguments":{}}]}
- The JSON fallback is executable only when the entire assistant message is that JSON object, optionally wrapped in one `json` code fence.
- Never write pseudo function syntax such as `memory_store(...)` or `tool_name(...)` in user-facing text.
- If the user asks you to show, repeat, quote, explain, or debug prompts, tool formats, JSON examples, or available tools, do not use the JSON fallback; answer as normal user-facing text.
- Only call tools listed in Available tools for this turn. Calls to any other tool will be rejected.
- If no tool is needed, answer normally in natural language.
- Never put raw tool-call JSON, function names, or hidden arguments in the final user-facing answer.
- After tool results are provided, produce only a natural-language answer for the user.
- If a tool result says confirmation is required, ask the user to confirm before claiming the action was done.
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
        channel_id: Some(channel_id.to_string()),
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
        let protocol = if response.tool_calls.is_empty() {
            response.tool_calls = parse_text_tool_calls(&response.content)?;
            if !response.tool_calls.is_empty() {
                response.content.clear();
            }
            ToolCallProtocol::TextFallback
        } else {
            ToolCallProtocol::Native
        };

        if response.tool_calls.is_empty() {
            return Ok(response.content);
        }

        let tool_calls = response
            .tool_calls
            .iter()
            .enumerate()
            .map(|(index, call)| normalize_tool_call(call, depth, index))
            .collect::<Vec<_>>();

        debug_component_tool_calls(
            debug_output,
            depth + 1,
            &tool_calls,
            Some((channel_id, user_id_text)),
        );

        messages.push(tool_call_request_message(
            protocol,
            &response.content,
            &tool_calls,
        ));

        for call in &tool_calls {
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
            messages.push(tool_result_message(protocol, &call.tool_name, result)?);
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
    user_text: &str,
    tools_config: &ToolsSection,
    channel_id: &str,
) -> anyhow::Result<Vec<ToolDefinition>> {
    if allowed_names.is_empty() {
        return Ok(Vec::new());
    }

    select_tools(
        modules,
        Some(allowed_names),
        user_text,
        tools_config,
        channel_id,
    )
    .await
}

pub(super) async fn select_all_tools(
    modules: &ModuleRegistry,
    user_text: &str,
    tools_config: &ToolsSection,
    channel_id: &str,
) -> anyhow::Result<Vec<ToolDefinition>> {
    select_tools(modules, None, user_text, tools_config, channel_id).await
}

async fn select_tools(
    modules: &ModuleRegistry,
    allowed_names: Option<&[String]>,
    user_text: &str,
    tools_config: &ToolsSection,
    channel_id: &str,
) -> anyhow::Result<Vec<ToolDefinition>> {
    let all_tools = modules.tools.list_tools().await?;
    let skill_scoped = allowed_names.is_some();
    let allowed = allowed_names.map(|allowed_names| {
        allowed_names
            .iter()
            .map(|name| name.as_str())
            .collect::<HashSet<_>>()
    });

    let tools = all_tools
        .into_iter()
        .filter(|tool| {
            if !tools_config.allow_plugin_tools && is_plugin_source(&tool.source) {
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

    let total = tools.len();
    let selected = rank_and_limit_tools(
        tools,
        user_text,
        tools_config.max_tools_per_turn,
        skill_scoped,
        channel_id,
    );
    if selected.len() < total {
        tracing::debug!(
            total,
            selected = selected.len(),
            skill_scoped,
            "tool router selected bounded tool set for turn"
        );
    }

    Ok(selected)
}

fn rank_and_limit_tools(
    tools: Vec<ToolDefinition>,
    user_text: &str,
    max_tools_per_turn: usize,
    skill_scoped: bool,
    channel_id: &str,
) -> Vec<ToolDefinition> {
    if tools.is_empty() || max_tools_per_turn == 0 {
        return Vec::new();
    }

    if skill_scoped && tools.len() <= max_tools_per_turn {
        return tools;
    }

    let query = user_text.to_lowercase();
    let query_terms = tokenize_for_tool_routing(user_text);
    let mut ranked = tools
        .into_iter()
        .enumerate()
        .map(|(index, tool)| {
            let score = score_tool_candidate(&tool, &query, &query_terms, skill_scoped, channel_id);
            (score, index, tool)
        })
        .filter(|(score, _, _)| skill_scoped || *score > 0)
        .collect::<Vec<_>>();

    ranked.sort_by(
        |(left_score, left_index, left_tool), (right_score, right_index, right_tool)| {
            right_score
                .cmp(left_score)
                .then_with(|| {
                    left_tool
                        .require_confirmation
                        .cmp(&right_tool.require_confirmation)
                })
                .then_with(|| left_index.cmp(right_index))
        },
    );

    ranked
        .into_iter()
        .take(max_tools_per_turn)
        .map(|(_, _, tool)| tool)
        .collect()
}

fn score_tool_candidate(
    tool: &ToolDefinition,
    query: &str,
    query_terms: &HashSet<String>,
    skill_scoped: bool,
    channel_id: &str,
) -> i32 {
    let mut score = if skill_scoped { 1 } else { 0 };
    let name = tool.name.to_lowercase();
    let searchable_text = format!(
        "{} {} {} {}",
        name.replace(['_', '-', '.'], " "),
        tool.description.to_lowercase(),
        tool.source.to_lowercase(),
        schema_property_names(&tool.parameters)
            .join(" ")
            .to_lowercase()
    );
    let searchable_terms = tokenize_for_tool_routing(&searchable_text);

    for term in query_terms {
        let term = term.as_str();
        if name == term {
            score += 16;
        } else if name.contains(term) {
            score += 8;
        }
        if searchable_terms.contains(term) {
            score += 3;
        }
    }

    for hint in tool_routing_hints(&name) {
        if query.contains(hint) {
            score += 12;
        }
    }

    if query.contains("http://") || query.contains("https://") {
        if name == "fetch" {
            score += 24;
        }
    }
    if looks_like_arithmetic(query) && name == "calculate" {
        score += 18;
    }
    if channel_id == "embedded" && name == "embedded_volume_control" {
        score += 8;
    }
    if name == "embedded_volume_control" && looks_like_embedded_volume_request(query) {
        score += 28;
    }
    if name == "device_control" && looks_like_embedded_volume_request(query) {
        score -= 8;
    }

    score
}

fn tokenize_for_tool_routing(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter_map(|term| {
            let term = term.trim();
            (term.len() >= 3).then(|| term.to_string())
        })
        .collect()
}

fn schema_property_names(parameters: &serde_json::Value) -> Vec<String> {
    parameters
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default()
}

fn tool_routing_hints(tool_name: &str) -> &'static [&'static str] {
    match tool_name {
        "get_current_time" => &[
            "time", "date", "today", "tomorrow", "now", "clock", "timezone", "when", "时间",
            "几点", "今天", "明天", "现在", "日期",
        ],
        "fetch" => &[
            "fetch",
            "url",
            "http",
            "https",
            "website",
            "web",
            "page",
            "link",
            "download",
            "search",
            "lookup",
            "look up",
            "research",
            "latest",
            "current",
            "news",
            "网页",
            "链接",
            "网址",
            "访问",
            "抓取",
            "搜索",
            "查一下",
            "查询",
            "检索",
            "查找",
            "最新",
            "新闻",
            "资料",
            "当前",
        ],
        "calculate" => &[
            "calculate",
            "compute",
            "math",
            "sum",
            "plus",
            "minus",
            "multiply",
            "divide",
            "算",
            "计算",
            "加",
            "减",
            "乘",
            "除",
        ],
        "memory_search" => &[
            "remember",
            "memory",
            "recall",
            "preference",
            "past",
            "记得",
            "记忆",
            "偏好",
            "喜欢",
            "之前",
        ],
        "memory_store" => &[
            "remember this",
            "likes",
            "favorite",
            "preference",
            "save",
            "store",
            "note",
            "memorize",
            "记住",
            "保存",
            "记录",
            "喜欢",
            "最喜欢",
            "偏好",
        ],
        "schedule_task" => &[
            "schedule", "remind", "reminder", "timer", "alarm", "cron", "later", "提醒", "定时",
            "闹钟", "日程",
        ],
        "send_notification" => &[
            "notify",
            "notification",
            "alert",
            "message",
            "push",
            "通知",
            "提醒",
        ],
        "device_control" => &[
            "device",
            "light",
            "ac",
            "air conditioner",
            "fan",
            "turn on",
            "turn off",
            "brightness",
            "temperature",
            "volume",
            "speaker",
            "设备",
            "灯",
            "空调",
            "风扇",
            "打开",
            "关闭",
            "音量",
            "扬声器",
            "温度",
        ],
        "embedded_volume_control" => &[
            "embedded",
            "pi",
            "raspberry",
            "mic",
            "microphone",
            "capture volume",
            "speaker",
            "playback volume",
            "louder",
            "quieter",
            "volume",
            "麦克风",
            "话筒",
            "拾音",
            "收音",
            "输入音量",
            "喇叭",
            "扬声器",
            "播放音量",
            "音量",
            "声音",
            "调大",
            "调小",
            "放大",
            "小声",
        ],
        "sensor_read" => &[
            "sensor",
            "temperature",
            "humidity",
            "value",
            "read",
            "status",
            "传感器",
            "温度",
            "湿度",
            "读数",
            "状态",
        ],
        "sensor_list" => &[
            "sensor",
            "sensors",
            "list sensors",
            "all sensors",
            "online",
            "offline",
            "传感器",
            "列表",
        ],
        _ => &[],
    }
}

fn looks_like_embedded_volume_request(query: &str) -> bool {
    let has_volume = [
        "volume", "louder", "quieter", "音量", "声音", "调大", "调小", "放大", "小声",
    ]
    .iter()
    .any(|hint| query.contains(hint));
    let has_embedded_audio_target = [
        "mic",
        "microphone",
        "speaker",
        "capture",
        "playback",
        "pi",
        "raspberry",
        "embedded",
        "麦克风",
        "话筒",
        "拾音",
        "收音",
        "扬声器",
        "喇叭",
    ]
    .iter()
    .any(|hint| query.contains(hint));

    has_volume && has_embedded_audio_target
}

fn looks_like_arithmetic(query: &str) -> bool {
    let has_operator = query
        .chars()
        .any(|ch| matches!(ch, '+' | '*' | '/' | '^' | '='))
        || query.contains(" - ");
    let digit_count = query.chars().filter(|ch| ch.is_ascii_digit()).count();
    has_operator && digit_count >= 2
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

fn tool_call_request_message(
    protocol: ToolCallProtocol,
    content: &str,
    calls: &[LlmToolCall],
) -> ChatMessage {
    match protocol {
        ToolCallProtocol::Native => ChatMessage {
            role: "assistant".into(),
            content: content.to_string(),
            name: None,
            tool_calls: calls.to_vec(),
            ..ChatMessage::default()
        },
        ToolCallProtocol::TextFallback => {
            let names = calls
                .iter()
                .map(|call| call.tool_name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            ChatMessage::new(
                "assistant",
                format!("Tool call requested through text protocol: {names}"),
            )
        }
    }
}

fn tool_result_message(
    protocol: ToolCallProtocol,
    tool_name: &str,
    result: ToolResult,
) -> anyhow::Result<ChatMessage> {
    let content = serde_json::to_string(&serde_json::json!({
        "tool": tool_name,
        "success": result.success,
        "output": result.output,
        "display": result.display,
    }))?;

    match protocol {
        ToolCallProtocol::Native => Ok(ChatMessage {
            role: "tool".into(),
            content,
            name: Some(tool_name.to_string()),
            tool_call_id: Some(result.call_id),
            ..ChatMessage::default()
        }),
        ToolCallProtocol::TextFallback => Ok(ChatMessage::new(
            "system",
            format!(
                "Tool result for text-protocol call `{}` (call_id `{}`): {}\nUse this result to answer the user. If confirmation is required, ask the user to confirm before claiming the action was done.",
                tool_name, result.call_id, content
            ),
        )),
    }
}

pub(super) fn parse_text_tool_calls(content: &str) -> anyhow::Result<Vec<LlmToolCall>> {
    let Some(candidate) = standalone_tool_call_json(content) else {
        return Ok(parse_pseudo_function_tool_call(content)
            .into_iter()
            .collect());
    };
    if !candidate.contains("\"tool_calls\"") {
        return Ok(Vec::new());
    }

    let value: serde_json::Value = match serde_json::from_str(candidate) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "ignoring malformed standalone text tool-call envelope"
            );
            return Ok(Vec::new());
        }
    };

    let Some(object) = value.as_object() else {
        return Ok(Vec::new());
    };
    if object.get("schema").and_then(|value| value.as_str()) != Some(TEXT_TOOL_CALL_SCHEMA) {
        tracing::warn!(
            schema = ?object.get("schema"),
            expected = TEXT_TOOL_CALL_SCHEMA,
            "ignoring text tool-call envelope with missing or invalid schema"
        );
        return Ok(Vec::new());
    };
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "schema" | "tool_calls"))
    {
        tracing::warn!("ignoring text tool-call envelope with unexpected top-level fields");
        return Ok(Vec::new());
    }

    let Some(calls) = object.get("tool_calls").and_then(|value| value.as_array()) else {
        tracing::warn!("ignoring text tool-call envelope without a tool_calls array");
        return Ok(Vec::new());
    };
    let mut parsed = Vec::new();
    for (index, call) in calls.iter().take(MAX_TEXT_TOOL_CALLS).enumerate() {
        let Some(call_object) = call.as_object() else {
            tracing::warn!(index, "ignoring non-object text tool call");
            continue;
        };
        if call_object
            .keys()
            .any(|key| !matches!(key.as_str(), "id" | "name" | "tool_name" | "arguments"))
        {
            tracing::warn!(index, "ignoring text tool call with unexpected fields");
            continue;
        }
        let Some(name) = call
            .get("name")
            .or_else(|| call.get("tool_name"))
            .and_then(|value| value.as_str())
            .filter(|name| is_valid_text_tool_name(name))
        else {
            tracing::warn!(index, "ignoring text tool call without a valid name");
            continue;
        };
        let arguments = call
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let arguments = match arguments {
            serde_json::Value::String(raw) => match serde_json::from_str::<serde_json::Value>(&raw)
            {
                Ok(value) if value.is_object() => value,
                Ok(_) => {
                    tracing::warn!(
                        index,
                        tool = %name,
                        "ignoring text tool call with non-object JSON arguments"
                    );
                    continue;
                }
                Err(error) => {
                    tracing::warn!(
                        index,
                        tool = %name,
                        error = %error,
                        "ignoring text tool call with invalid JSON arguments"
                    );
                    continue;
                }
            },
            value if value.is_object() => value,
            _ => {
                tracing::warn!(
                    index,
                    tool = %name,
                    "ignoring text tool call with non-object arguments"
                );
                continue;
            }
        };
        let id = call
            .get("id")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_default();
        parsed.push(LlmToolCall {
            id,
            tool_name: name.to_string(),
            arguments,
        });
    }
    if calls.len() > MAX_TEXT_TOOL_CALLS {
        tracing::warn!(
            count = calls.len(),
            limit = MAX_TEXT_TOOL_CALLS,
            "truncated oversized text tool-call envelope"
        );
    }

    Ok(parsed)
}

fn parse_pseudo_function_tool_call(content: &str) -> Option<LlmToolCall> {
    let trimmed = content.trim_start();
    let name_end = trimmed
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        .last()
        .map(|(index, ch)| index + ch.len_utf8())?;
    let name = &trimmed[..name_end];
    if !is_valid_text_tool_name(name) {
        return None;
    }

    let rest = trimmed[name_end..].trim_start();
    if !rest.starts_with('(') {
        return None;
    }

    let closing = find_matching_paren(rest)?;
    let args = &rest[1..closing];
    let arguments = parse_pseudo_function_arguments(args).ok()?;
    Some(LlmToolCall {
        id: String::new(),
        tool_name: name.to_string(),
        arguments,
    })
}

fn find_matching_paren(input: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;

    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = true;
                quote = ch;
            }
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }

    None
}

fn parse_pseudo_function_arguments(input: &str) -> anyhow::Result<serde_json::Value> {
    let mut object = serde_json::Map::new();
    for item in split_pseudo_args(input) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let Some((key, value)) = split_pseudo_key_value(item) else {
            anyhow::bail!("invalid pseudo tool argument '{item}'");
        };
        let key = key.trim();
        if !is_valid_text_tool_name(key) {
            anyhow::bail!("invalid pseudo tool argument key '{key}'");
        }
        object.insert(key.to_string(), parse_pseudo_value(value.trim()));
    }

    Ok(serde_json::Value::Object(object))
}

fn split_pseudo_args(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;
    let mut bracket_depth = 0usize;

    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = true;
                quote = ch;
            }
            '[' | '{' => bracket_depth += 1,
            ']' | '}' => bracket_depth = bracket_depth.saturating_sub(1),
            ',' if bracket_depth == 0 => {
                parts.push(&input[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&input[start..]);
    parts
}

fn split_pseudo_key_value(input: &str) -> Option<(&str, &str)> {
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;

    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = true;
                quote = ch;
            }
            '=' | ':' => return Some((&input[..index], &input[index + ch.len_utf8()..])),
            _ => {}
        }
    }

    None
}

fn parse_pseudo_value(input: &str) -> serde_json::Value {
    let trimmed = input.trim();
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        return parse_quoted_pseudo_string(trimmed)
            .map(serde_json::Value::String)
            .unwrap_or_else(|| serde_json::Value::String(trimmed.to_string()));
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return value;
    }
    if let Ok(number) = trimmed.parse::<f64>() {
        return serde_json::json!(number);
    }
    serde_json::Value::String(trimmed.to_string())
}

fn parse_quoted_pseudo_string(input: &str) -> Option<String> {
    let quote = input.chars().next()?;
    let mut value = String::new();
    let mut escaped = false;

    for ch in input[quote.len_utf8()..].chars() {
        if escaped {
            let decoded = match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '"' => '"',
                '\'' => '\'',
                other => other,
            };
            value.push(decoded);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return Some(value);
        } else {
            value.push(ch);
        }
    }

    None
}

fn standalone_tool_call_json(content: &str) -> Option<&str> {
    let trimmed = content.trim();

    if trimmed.starts_with("```") {
        return standalone_json_fence(trimmed);
    }

    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        Some(trimmed)
    } else {
        None
    }
}

fn standalone_json_fence(trimmed: &str) -> Option<&str> {
    let body = trimmed.strip_prefix("```")?;
    let header_end = body.find('\n')?;
    let header = body[..header_end].trim();
    if !header.is_empty() && !header.eq_ignore_ascii_case("json") {
        return None;
    }

    let after_header = &body[header_end + 1..];
    let closing_start = after_header.rfind("```")?;
    if !after_header[closing_start + 3..].trim().is_empty() {
        return None;
    }
    let inner = after_header[..closing_start].trim();
    (inner.starts_with('{') && inner.ends_with('}')).then_some(inner)
}

fn is_valid_text_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    name.len() <= 128 && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "timezone": {"type": "string"}
                }
            }),
            source: "builtin".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    #[test]
    fn routes_global_tools_by_query_relevance() {
        let selected = rank_and_limit_tools(
            vec![
                tool("device_control", "Control smart home devices"),
                tool("get_current_time", "Get the current date and time"),
                tool("fetch", "Fetch a public HTTP URL"),
            ],
            "what time is it now?",
            2,
            false,
            "cli",
        );

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "get_current_time");
    }

    #[test]
    fn avoids_global_tool_injection_when_no_tool_matches() {
        let selected = rank_and_limit_tools(
            vec![
                tool("device_control", "Control smart home devices"),
                tool("fetch", "Fetch a public HTTP URL"),
            ],
            "tell me a bedtime story",
            8,
            false,
            "cli",
        );

        assert!(selected.is_empty());
    }

    #[test]
    fn skill_scoped_tools_are_capped_but_not_dropped_for_zero_score() {
        let selected = rank_and_limit_tools(
            vec![
                tool("skill_tool_a", "A domain tool"),
                tool("skill_tool_b", "Another domain tool"),
                tool("skill_tool_c", "Third domain tool"),
            ],
            "unrelated input",
            2,
            true,
            "cli",
        );

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].name, "skill_tool_a");
        assert_eq!(selected[1].name, "skill_tool_b");
    }

    #[test]
    fn routes_preference_statements_to_memory_store() {
        let selected = rank_and_limit_tools(
            vec![
                tool("memory_search", "Search the user's memory"),
                tool("memory_store", "Store a piece of information to memory"),
                tool("fetch", "Fetch a public HTTP URL"),
            ],
            "KeQiao 非常喜欢日本音乐家 tsukuyomi",
            8,
            false,
            "cli",
        );

        assert!(selected.iter().any(|tool| tool.name == "memory_store"));
    }

    #[test]
    fn routes_embedded_mic_volume_to_embedded_volume_tool() {
        let selected = rank_and_limit_tools(
            vec![
                tool("device_control", "Control smart home devices"),
                tool(
                    "embedded_volume_control",
                    "Get or set microphone and speaker volume for an alia-embedded channel",
                ),
            ],
            "把麦克风音量调整到2",
            1,
            false,
            "embedded",
        );

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "embedded_volume_control");
    }

    #[test]
    fn routes_search_language_to_fetch_candidate() {
        let selected = rank_and_limit_tools(
            vec![
                tool("device_control", "Control smart home devices"),
                tool("fetch", "Fetch a public HTTP URL"),
            ],
            "帮我搜索一下 sherpa onnx 最新版本",
            8,
            false,
            "cli",
        );

        assert!(selected.iter().any(|tool| tool.name == "fetch"));
    }

    #[test]
    fn native_tool_history_uses_normalized_call_ids() {
        let call = normalize_tool_call(
            &LlmToolCall {
                id: String::new(),
                tool_name: "device_control".to_string(),
                arguments: serde_json::json!({"kind": "microphone", "volume": 2.0}),
            },
            0,
            0,
        );
        let request =
            tool_call_request_message(ToolCallProtocol::Native, "", std::slice::from_ref(&call));
        let result = tool_result_message(
            ToolCallProtocol::Native,
            "device_control",
            ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: serde_json::json!({"requires_confirmation": true}),
                display: Some("Please confirm".to_string()),
            },
        )
        .unwrap();

        assert_eq!(request.tool_calls[0].id, "call_0_0");
        assert_eq!(result.role, "tool");
        assert_eq!(result.tool_call_id.as_deref(), Some("call_0_0"));
    }

    #[test]
    fn text_fallback_tool_history_does_not_use_native_tool_roles() {
        let call = normalize_tool_call(
            &LlmToolCall {
                id: String::new(),
                tool_name: "device_control".to_string(),
                arguments: serde_json::json!({"kind": "microphone", "volume": 2.0}),
            },
            0,
            0,
        );
        let request = tool_call_request_message(
            ToolCallProtocol::TextFallback,
            "",
            std::slice::from_ref(&call),
        );
        let result = tool_result_message(
            ToolCallProtocol::TextFallback,
            "device_control",
            ToolResult {
                call_id: call.id,
                success: false,
                output: serde_json::json!({"requires_confirmation": true}),
                display: Some("Please confirm".to_string()),
            },
        )
        .unwrap();

        assert_eq!(request.role, "assistant");
        assert!(request.tool_calls.is_empty());
        assert_eq!(result.role, "system");
        assert!(result.tool_call_id.is_none());
        assert!(result.content.contains("requires_confirmation"));
    }
}
