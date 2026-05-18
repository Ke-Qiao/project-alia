use alia_server::channels::pipeline::tools::{
    ToolCallProtocol, non_empty_tool_fallback, normalize_tool_call, rank_and_limit_tools,
    tool_call_request_message, tool_result_message,
};
use alia_server::providers::adapter::{LlmToolCall, ToolDefinition, ToolResult};

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
        routing_hints: Vec::new(),
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
    let mut volume_tool = tool(
        "embedded_volume_control",
        "Get or set microphone and speaker volume for an alia-embedded channel",
    );
    volume_tool.routing_hints = vec![
        "microphone".to_string(),
        "speaker".to_string(),
        "volume".to_string(),
        "麦克风".to_string(),
        "音量".to_string(),
    ];
    let selected = rank_and_limit_tools(
        vec![
            tool("device_control", "Control smart home devices"),
            volume_tool,
        ],
        "把麦克风音量调整到2",
        1,
        false,
        "pi-living-room",
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

#[test]
fn empty_tool_response_falls_back_to_last_tool_display() {
    assert_eq!(
        non_empty_tool_fallback(Some("Updated embedded volume: mic 1.00")),
        "Updated embedded volume: mic 1.00"
    );
}

#[test]
fn empty_tool_response_has_non_empty_generic_fallback() {
    assert!(!non_empty_tool_fallback(None).trim().is_empty());
    assert!(!non_empty_tool_fallback(Some("   ")).trim().is_empty());
}
