use std::sync::Arc;

use alia_server::providers::adapter::{MemoryBackend, ToolContext, ToolDefinition, ToolExecutor};
use alia_server::providers::memory::simple::AliaMemory;
#[cfg(feature = "tools-http")]
use alia_server::tools::builtin::FetchUrl;
use alia_server::tools::builtin::{
    Calculate, GetCurrentTime, MemoryForget, MemorySearch, MemoryStore, parse_openai_tool_call,
    to_openai_functions,
};
use uuid::Uuid;

#[tokio::test]
async fn test_get_current_time() {
    let tool = GetCurrentTime;
    let ctx = ToolContext {
        context_id: None,
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: None,
        source_event_id: None,
        request_id: uuid::Uuid::new_v4(),
        invocation_key: "0".into(),
        cancel: None,
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
    assert_eq!(result.output["timezone"], "UTC");
    assert_eq!(result.output["utc_offset"], "+00:00");
}

#[tokio::test]
async fn test_get_current_time_uses_iana_timezone() {
    let tool = GetCurrentTime;
    let ctx = ToolContext {
        context_id: None,
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: None,
        source_event_id: None,
        request_id: uuid::Uuid::new_v4(),
        invocation_key: "0".into(),
        cancel: None,
        user_id: Uuid::new_v4(),
        conversation_id: None,
        source: "test".to_string(),
        channel_id: None,
    };

    let result = tool
        .execute(
            serde_json::json!({"format": "human", "timezone": "Asia/Shanghai"}),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.success);
    assert_eq!(result.output["timezone"], "Asia/Shanghai");
    assert_eq!(result.output["utc_offset"], "+08:00");
    assert!(
        result
            .display
            .as_deref()
            .is_some_and(|display| display.contains("+08:00"))
    );
}

#[tokio::test]
async fn test_get_current_time_rejects_invalid_timezone() {
    let tool = GetCurrentTime;
    let ctx = ToolContext {
        context_id: None,
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: None,
        source_event_id: None,
        request_id: uuid::Uuid::new_v4(),
        invocation_key: "0".into(),
        cancel: None,
        user_id: Uuid::new_v4(),
        conversation_id: None,
        source: "test".to_string(),
        channel_id: None,
    };

    let error = tool
        .execute(
            serde_json::json!({"format": "human", "timezone": "Shanghai"}),
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("invalid timezone"));
}

#[tokio::test]
async fn test_calculate_expression() {
    let tool = Calculate;
    let ctx = ToolContext {
        context_id: None,
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: None,
        source_event_id: None,
        request_id: uuid::Uuid::new_v4(),
        invocation_key: "0".into(),
        cancel: None,
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
        context_id: None,
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: None,
        source_event_id: None,
        request_id: uuid::Uuid::new_v4(),
        invocation_key: "0".into(),
        cancel: None,
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
        routing_hints: Vec::new(),
        source: "builtin".to_string(),
        require_confirmation: false,
        required_role: None,
    }];

    let functions = to_openai_functions(&tools);
    assert_eq!(functions.len(), 1);
    assert_eq!(functions[0]["function"]["name"], "test_tool");
}

#[test]
fn memory_forget_schema_is_openai_compatible() {
    let definition = MemoryForget::new(None).definition();

    assert_eq!(definition.parameters["type"], "object");
    for key in ["oneOf", "anyOf", "allOf", "enum", "not"] {
        assert!(
            definition.parameters.get(key).is_none(),
            "memory_forget must not expose top-level {key} in OpenAI function parameters"
        );
    }
    assert!(definition.parameters["properties"]["id"].is_object());
    assert!(definition.parameters["properties"]["query"].is_object());
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

#[tokio::test]
async fn memory_store_and_search_use_memory_system() {
    let memory: Arc<dyn MemoryBackend> = Arc::new(AliaMemory::new());
    let user_id = Uuid::new_v4();
    let epoch = memory.write_snapshot(user_id).await.unwrap().privacy_epoch;
    let ctx = ToolContext {
        context_id: None,
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: Some(epoch),
        source_event_id: None,
        request_id: uuid::Uuid::new_v4(),
        invocation_key: "0".into(),
        cancel: None,
        user_id,
        conversation_id: None,
        source: "test".to_string(),
        channel_id: Some("test".to_string()),
    };
    let store = MemoryStore::new(Some(memory.clone()));
    let search = MemorySearch::new(Some(memory));

    let stored = store
        .execute(
            serde_json::json!({
                "content": "KeQiao likes tsukuyomi",
                "layer": "core",
                "importance": 1.0
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(stored.success);
    assert_eq!(stored.output["stored"], true);

    let found = search
        .execute(
            serde_json::json!({
                "query": "tsukuyomi",
                "layers": ["core"]
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(found.success);
    assert_eq!(found.output["count"], 1);
    assert_eq!(
        found.output["results"][0]["content"],
        "KeQiao likes tsukuyomi"
    );
}

#[tokio::test]
async fn memory_store_blocks_sensitive_chat_memory() {
    let memory: Arc<dyn MemoryBackend> = Arc::new(AliaMemory::new());
    let user_id = Uuid::new_v4();
    let epoch = memory.write_snapshot(user_id).await.unwrap().privacy_epoch;
    let ctx = ToolContext {
        context_id: None,
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: Some(epoch),
        source_event_id: None,
        request_id: uuid::Uuid::new_v4(),
        invocation_key: "0".into(),
        cancel: None,
        user_id,
        conversation_id: None,
        source: "test".to_string(),
        channel_id: Some("test".to_string()),
    };
    let store = MemoryStore::new(Some(memory.clone()));

    let result = store
        .execute(
            serde_json::json!({
                "content": "The user's API key is sk-live-12345678901234567890abcdef.",
                "layer": "core",
                "importance": 1.0
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.success);
    assert_eq!(result.output["error"], "sensitive_memory_blocked");
    assert!(
        memory
            .list(ctx.user_id, Default::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn old_memory_tool_cannot_recreate_or_update_memories_after_clear() {
    let memory = Arc::new(AliaMemory::new());
    let store = MemoryStore::new(Some(memory.clone()));
    let user_id = Uuid::new_v4();
    let epoch = memory.write_snapshot(user_id).await.unwrap().privacy_epoch;
    let old = ToolContext {
        context_id: None,
        user_id,
        request_id: Uuid::new_v4(),
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: Some(epoch),
        source_event_id: None,
        invocation_key: "0".into(),
        conversation_id: None,
        source: "manual".into(),
        channel_id: None,
        cancel: None,
    };
    let original =
        serde_json::json!({"content": "The user prefers concise replies.", "layer": "preference"});
    store.execute(original.clone(), &old).await.unwrap();
    memory.clear(old.user_id).await.unwrap();
    assert!(store.execute(original.clone(), &old).await.is_err());
    assert!(
        memory
            .list(old.user_id, Default::default())
            .await
            .unwrap()
            .is_empty()
    );

    let fresh = ToolContext {
        context_id: None,
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: Some(memory.write_snapshot(user_id).await.unwrap().privacy_epoch),
        request_id: Uuid::new_v4(),
        ..old.clone()
    };
    store.execute(original, &fresh).await.unwrap();
    assert!(store.execute(serde_json::json!({"content": "The user prefers detailed replies.", "layer": "preference"}), &old).await.is_err());
    let saved = memory.list(old.user_id, Default::default()).await.unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].content, "The user prefers concise replies.");
    let update = store.execute(serde_json::json!({"content": "The user prefers detailed replies.", "layer": "preference"}), &fresh).await.unwrap();
    assert_eq!(update.output["action"], "updated");
    let saved = memory.list(old.user_id, Default::default()).await.unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].content, "The user prefers detailed replies.");
}

#[tokio::test]
async fn ai_memory_tool_requires_a_persisted_source_message() {
    let memory = Arc::new(AliaMemory::new());
    let store = MemoryStore::new(Some(memory.clone()));
    let user_id = Uuid::new_v4();
    let epoch = memory.write_snapshot(user_id).await.unwrap().privacy_epoch;
    let context = ToolContext {
        context_id: None,
        user_id,
        request_id: Uuid::new_v4(),
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: Some(epoch),
        source_event_id: None,
        invocation_key: "0".into(),
        conversation_id: None,
        source: "ai".into(),
        channel_id: None,
        cancel: None,
    };
    let error = store
        .execute(
            serde_json::json!({"content":"The user likes green tea."}),
            &context,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("persisted source user message"));
    assert!(
        memory
            .list(context.user_id, Default::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn memory_forget_deletes_matching_memory() {
    let memory: Arc<dyn MemoryBackend> = Arc::new(AliaMemory::new());
    let user_id = Uuid::new_v4();
    let epoch = memory.write_snapshot(user_id).await.unwrap().privacy_epoch;
    let ctx = ToolContext {
        context_id: None,
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: Some(epoch),
        source_event_id: None,
        request_id: uuid::Uuid::new_v4(),
        invocation_key: "0".into(),
        cancel: None,
        user_id,
        conversation_id: None,
        source: "test".to_string(),
        channel_id: Some("test".to_string()),
    };
    let store = MemoryStore::new(Some(memory.clone()));
    let forget = MemoryForget::new(Some(memory.clone()));

    store
        .execute(
            serde_json::json!({
                "content": "The user likes short summaries.",
                "layer": "preference",
                "importance": 0.8
            }),
            &ctx,
        )
        .await
        .unwrap();

    let result = forget
        .execute(serde_json::json!({"query": "short summaries"}), &ctx)
        .await
        .unwrap();

    assert!(result.success);
    assert_eq!(result.output["deleted"], 1);
    assert!(
        memory
            .list(ctx.user_id, Default::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn memory_forget_batch_has_one_privacy_transition_and_rejects_old_request() {
    use alia_server::providers::adapter::MemoryNode;
    let memory = Arc::new(AliaMemory::new());
    let user_id = Uuid::new_v4();
    for content in [
        "The user drinks green tea",
        "The user drinks black tea",
        "The user drinks coffee",
    ] {
        memory
            .store(user_id, MemoryNode::new("core", "fact", content, 0.8))
            .await
            .unwrap();
    }
    let ctx = ToolContext {
        context_id: None,
        source_received_at: chrono::Utc::now(),
        source_privacy_epoch: Some(memory.write_snapshot(user_id).await.unwrap().privacy_epoch),
        source_event_id: None,
        request_id: Uuid::new_v4(),
        invocation_key: "0".into(),
        cancel: None,
        user_id,
        conversation_id: None,
        source: "test".into(),
        channel_id: None,
    };
    let forget = MemoryForget::new(Some(memory.clone()));
    let epoch = memory.write_snapshot(user_id).await.unwrap().privacy_epoch;
    let result = forget
        .execute(serde_json::json!({"query":"tea", "max_results":2}), &ctx)
        .await
        .unwrap();
    assert_eq!(result.output["selected"], 2);
    assert_eq!(result.output["deleted"], 2);
    assert_eq!(
        memory.write_snapshot(user_id).await.unwrap().privacy_epoch,
        epoch + 1
    );
    let kept = memory.list(user_id, Default::default()).await.unwrap();
    assert_eq!(kept.len(), 1);
    assert!(kept[0].content.contains("coffee"));

    // A delayed old tool request must not refresh its source time to erase the
    // user's later memory, even though its selection happens after the clear.
    memory.clear(user_id).await.unwrap();
    let id = memory
        .store(
            user_id,
            MemoryNode::new("core", "fact", "New coffee preference", 0.8),
        )
        .await
        .unwrap();
    assert!(
        forget
            .execute(serde_json::json!({"query":"coffee"}), &ctx)
            .await
            .is_err()
    );
    assert!(memory.get(user_id, &id).await.unwrap().is_some());
}
