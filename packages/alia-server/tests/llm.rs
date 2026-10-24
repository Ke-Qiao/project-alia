use alia_server::providers::adapter::ChatMessage;

#[test]
fn persisted_context_author_survives_retired_provider_message_fields() {
    let previous = serde_json::json!({
        "role": "user",
        "content": "Hello",
        "name": "11111111-1111-4111-8111-111111111111",
        "tool_call_id": null,
        "tool_calls": null
    });
    let message: ChatMessage = serde_json::from_value(previous).unwrap();
    assert_eq!(
        message.user_id.as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );
    let current = serde_json::to_value(message).unwrap();
    assert_eq!(current["content"], "Hello");
    assert_eq!(current["user_id"], "11111111-1111-4111-8111-111111111111");
    assert!(current.get("name").is_none());
    assert!(current.get("tool_calls").is_none());
}
