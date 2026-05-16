use alia_server::providers::adapter::{ToolContext, ToolDefinition, ToolExecutor};
#[cfg(feature = "tools-http")]
use alia_server::tools::builtin::FetchUrl;
use alia_server::tools::builtin::{
    Calculate, GetCurrentTime, parse_openai_tool_call, to_openai_functions,
};
use uuid::Uuid;

#[tokio::test]
async fn test_get_current_time() {
    let tool = GetCurrentTime;
    let ctx = ToolContext {
        user_id: Uuid::new_v4(),
        conversation_id: None,
        source: "test".to_string(),
        channel_id: None,
    };

    let result = tool
        .execute(serde_json::json!({"format": "iso8601"}), &ctx)
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.get("datetime").is_some());
    assert!(result.output.get("timestamp").is_some());
}

#[tokio::test]
async fn test_calculate_expression() {
    let tool = Calculate;
    let ctx = ToolContext {
        user_id: Uuid::new_v4(),
        conversation_id: None,
        source: "test".to_string(),
        channel_id: None,
    };

    let result = tool
        .execute(serde_json::json!({"expression": "(2 + 3) * 4 - 2^3"}), &ctx)
        .await
        .unwrap();

    assert!(result.success);
    assert_eq!(result.output["display"], "12");
}

#[tokio::test]
async fn test_calculate_rejects_division_by_zero() {
    let tool = Calculate;
    let ctx = ToolContext {
        user_id: Uuid::new_v4(),
        conversation_id: None,
        source: "test".to_string(),
        channel_id: None,
    };

    let result = tool
        .execute(serde_json::json!({"expression": "1 / 0"}), &ctx)
        .await;

    assert!(result.is_err());
}

#[cfg(feature = "tools-http")]
#[test]
fn test_fetch_definition() {
    let definition = FetchUrl.definition();
    assert_eq!(definition.name, "fetch");
    assert!(!definition.require_confirmation);
    assert_eq!(definition.parameters["required"][0], "url");
}

#[test]
fn test_to_openai_functions() {
    let tools = vec![ToolDefinition {
        name: "test_tool".to_string(),
        description: "A test tool".to_string(),
        parameters: serde_json::json!({"type": "object"}),
        source: "builtin".to_string(),
        require_confirmation: false,
        required_role: None,
    }];

    let functions = to_openai_functions(&tools);
    assert_eq!(functions.len(), 1);
    assert_eq!(functions[0]["function"]["name"], "test_tool");
}

#[test]
fn test_parse_openai_tool_call() {
    let call_json = serde_json::json!({
        "id": "call_123",
        "name": "get_current_time",
        "arguments": "{\"format\": \"human\"}"
    });

    let call = parse_openai_tool_call(&call_json).unwrap();
    assert_eq!(call.tool_name, "get_current_time");
    assert_eq!(call.call_id, "call_123");
    assert_eq!(call.arguments["format"], "human");
}
