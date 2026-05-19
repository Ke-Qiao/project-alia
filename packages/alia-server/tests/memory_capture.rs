use std::pin::Pin;

use alia_server::channels::pipeline::memory_capture::{
    MemoryCandidateAction, MemoryCaptureStatus, capture_turn_memory_with_backends,
};
use alia_server::channels::pipeline::{CacheWrite, PendingTextPersistence};
use alia_server::config::{MemoryCaptureMode, MemorySection};
use alia_server::module::ModuleLifecycle;
use alia_server::providers::adapter::{
    ChatMessage, LlmAdapter, LlmResponse, MemoryBackend, MemoryListOpts,
};
use alia_server::providers::memory::SimpleMemory;
use async_trait::async_trait;
use futures::Stream;
use futures::stream;
use uuid::Uuid;

struct StaticLlm {
    response: String,
}

impl StaticLlm {
    fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
        }
    }
}

#[async_trait]
impl ModuleLifecycle for StaticLlm {
    fn name(&self) -> &'static str {
        "test_memory_capture_llm"
    }
}

#[async_trait]
impl LlmAdapter for StaticLlm {
    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String> {
        assert!(
            messages
                .iter()
                .any(|message| message.content.contains("Store only durable user facts")),
            "memory capture prompt should be explicit and English"
        );
        Ok(self.response.clone())
    }

    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        _tools: &[alia_server::providers::adapter::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(LlmResponse::text(self.chat(messages).await?))
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> {
        Ok(Box::pin(stream::once({
            let response = self.response.clone();
            async move { Ok(response) }
        })))
    }
}

fn pending_turn(user_id: Uuid) -> PendingTextPersistence {
    PendingTextPersistence {
        session_id: Uuid::new_v4(),
        user_id,
        user_id_text: "test-user".to_string(),
        query_text: "I prefer concise answers, but can you summarize this article now?".to_string(),
        response_text: "Sure, here is a concise summary.".to_string(),
        channel_id: "test-channel".to_string(),
        cache_write: None::<CacheWrite>,
    }
}

fn memory_config() -> MemorySection {
    MemorySection {
        enabled: true,
        capture_mode: MemoryCaptureMode::LlmSelected,
        capture_timeout_sec: 3,
        capture_min_confidence: 0.65,
        sensitive_filter_enabled: true,
        retrieval_layers: vec![
            "core".to_string(),
            "preference".to_string(),
            "relationship".to_string(),
        ],
        max_injected_memory_chars: 1200,
        max_retrieval_results: 10,
        similarity_threshold: 0.3,
        health_check_attempts: 1,
        health_check_interval_ms: 1,
    }
}

#[tokio::test]
async fn llm_selected_capture_stores_only_high_confidence_durable_memories() {
    let user_id = Uuid::new_v4();
    let pending = pending_turn(user_id);
    let memory = SimpleMemory::new();
    let llm = StaticLlm::new(
        r#"{"memories":[
            {
                "content":"The user prefers concise answers.",
                "memory_layer":"preference",
                "node_type":"preference",
                "importance":0.8,
                "confidence":0.92,
                "reason":"The user explicitly stated a durable response preference."
            },
            {
                "content":"The user is reading an article right now.",
                "memory_layer":"episodic",
                "node_type":"event",
                "importance":0.3,
                "confidence":0.31,
                "reason":"This is transient and uncertain."
            }
        ]}"#,
    );

    let trace = capture_turn_memory_with_backends(&llm, &memory, &pending, &memory_config()).await;

    assert_eq!(trace.status, MemoryCaptureStatus::Completed);
    assert_eq!(trace.candidates, 2);
    assert_eq!(trace.stored, 1);
    assert_eq!(trace.skipped, 1);
    assert_eq!(
        trace.candidate_traces[0].action,
        MemoryCandidateAction::Stored
    );
    assert_eq!(
        trace.candidate_traces[1].skip_reason.as_deref(),
        Some("low_confidence")
    );

    let stored = memory
        .list(user_id, MemoryListOpts::default())
        .await
        .expect("stored memories should list");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].content, "The user prefers concise answers.");
    assert_eq!(stored[0].memory_layer, "preference");
    assert_eq!(stored[0].node_type, "preference");
    assert_eq!(stored[0].metadata["capture_mode"], "llm_selected");
}

#[tokio::test]
async fn llm_selected_capture_does_not_store_when_model_returns_no_memories() {
    let user_id = Uuid::new_v4();
    let pending = pending_turn(user_id);
    let memory = SimpleMemory::new();
    let llm = StaticLlm::new(r#"{"memories":[]}"#);

    let trace = capture_turn_memory_with_backends(&llm, &memory, &pending, &memory_config()).await;

    assert_eq!(trace.status, MemoryCaptureStatus::Completed);
    assert_eq!(trace.candidates, 0);
    assert_eq!(trace.stored, 0);
    assert_eq!(trace.skipped, 0);
    assert!(
        memory
            .list(user_id, MemoryListOpts::default())
            .await
            .expect("stored memories should list")
            .is_empty()
    );
}

#[tokio::test]
async fn provider_infer_all_skips_backend_without_provider_inference() {
    let user_id = Uuid::new_v4();
    let pending = pending_turn(user_id);
    let memory = SimpleMemory::new();
    let llm = StaticLlm::new(r#"{"memories":[]}"#);
    let mut config = memory_config();
    config.capture_mode = MemoryCaptureMode::ProviderInferAll;

    let trace = capture_turn_memory_with_backends(&llm, &memory, &pending, &config).await;

    assert_eq!(trace.status, MemoryCaptureStatus::Skipped);
    assert_eq!(
        trace.reason.as_deref(),
        Some("provider_conversation_ingest_unsupported")
    );
    assert!(
        memory
            .list(user_id, MemoryListOpts::default())
            .await
            .expect("stored memories should list")
            .is_empty()
    );
}

#[tokio::test]
async fn llm_selected_capture_blocks_sensitive_memories() {
    let user_id = Uuid::new_v4();
    let pending = pending_turn(user_id);
    let memory = SimpleMemory::new();
    let llm = StaticLlm::new(
        r#"{"memories":[
            {
                "content":"The user's API key is sk-live-12345678901234567890abcdef.",
                "memory_layer":"core",
                "node_type":"fact",
                "importance":0.9,
                "confidence":0.99,
                "reason":"Sensitive credential."
            }
        ]}"#,
    );

    let trace = capture_turn_memory_with_backends(&llm, &memory, &pending, &memory_config()).await;

    assert_eq!(trace.status, MemoryCaptureStatus::Completed);
    assert_eq!(trace.stored, 0);
    assert_eq!(trace.skipped, 1);
    assert!(
        trace.candidate_traces[0]
            .skip_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("sensitive_memory_blocked"))
    );
    assert!(
        memory
            .list(user_id, MemoryListOpts::default())
            .await
            .expect("stored memories should list")
            .is_empty()
    );
}

#[tokio::test]
async fn llm_selected_capture_updates_existing_preference_memory() {
    let user_id = Uuid::new_v4();
    let pending = pending_turn(user_id);
    let memory = SimpleMemory::new();
    memory
        .store(
            user_id,
            alia_server::providers::adapter::MemoryNode {
                id: None,
                memory_layer: "preference".to_string(),
                node_type: "preference".to_string(),
                content: "The user prefers detailed replies.".to_string(),
                metadata: serde_json::json!({"source": "test"}),
                importance: 0.4,
            },
        )
        .await
        .unwrap();
    let llm = StaticLlm::new(
        r#"{"memories":[
            {
                "content":"The user prefers concise answers.",
                "memory_layer":"preference",
                "node_type":"preference",
                "importance":0.8,
                "confidence":0.95,
                "reason":"The user corrected their response style preference."
            }
        ]}"#,
    );

    let trace = capture_turn_memory_with_backends(&llm, &memory, &pending, &memory_config()).await;

    assert_eq!(trace.stored, 1);
    assert_eq!(
        trace.candidate_traces[0].action,
        MemoryCandidateAction::Updated
    );
    let stored = memory
        .list(user_id, MemoryListOpts::default())
        .await
        .expect("stored memories should list");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].content, "The user prefers concise answers.");
    assert_eq!(stored[0].importance, 0.8);
}
