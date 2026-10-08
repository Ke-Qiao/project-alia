use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU32, Ordering};

struct FixedMemoryLlm {
    response: String,
    calls: AtomicU32,
}

#[async_trait]
impl ModuleLifecycle for FixedMemoryLlm {
    fn name(&self) -> &'static str {
        "fixed_insights_memory_llm"
    }
}

#[async_trait]
impl LlmAdapter for FixedMemoryLlm {
    async fn chat(&self, _messages: &[ChatMessage]) -> anyhow::Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.response.clone())
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
    ) -> anyhow::Result<std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<String>> + Send>>>
    {
        anyhow::bail!("insights memory tests require the non-streaming call path")
    }
}

async fn consolidate_plan_fixture(
    memory: Option<Arc<MemoryService>>,
    llm: Option<Arc<dyn LlmAdapter>>,
    user: &MemoryMaintenanceUser,
    limit: u32,
    _config: &InsightsSection,
) -> anyhow::Result<MemoryMaintenanceStats> {
    // This fixture tests plan validation/atomic batches, not PG context admission.
    let memory = memory.unwrap();
    let llm = llm.unwrap();
    let mut context = memory.write_snapshot(user.user_id).await?;
    let memories = memory
        .list(
            user.user_id,
            MemoryListOpts {
                max_results: Some(limit),
                layers: None,
            },
        )
        .await?;
    context.source_memories = source_versions(&memories);
    let response = llm.chat(&[]).await?;
    apply_consolidation_response(
        memory.as_ref(),
        &context,
        &memories,
        user,
        &response,
        llm.name(),
    )
    .await
}

fn insight_user() -> MemoryMaintenanceUser {
    let user_id = Uuid::new_v4();
    MemoryMaintenanceUser {
        user_id,
        user_id_text: user_id.to_string(),
        channel_id: Some("insights-test".into()),
        session_id: None,
        last_seen_at: Utc::now(),
    }
}
#[tokio::test]
async fn consolidation_commits_multiple_updates_and_creates_in_one_snapshot() {
    let memory = Arc::new(MemoryService::new());
    let user = insight_user();
    let first = memory
        .store(
            user.user_id,
            MemoryNode::new("core", "fact", "First original fact", 0.8),
        )
        .await
        .unwrap();
    let second = memory
        .store(
            user.user_id,
            MemoryNode::new("core", "fact", "Second original fact", 0.8),
        )
        .await
        .unwrap();
    let original_epoch = memory
        .write_snapshot(user.user_id)
        .await
        .unwrap()
        .privacy_epoch;
    let llm = Arc::new(FixedMemoryLlm {
        response: serde_json::json!({
            "summary": "Everything has been consolidated successfully",
            "updates": [
                {"id": first, "content": "First revised fact"},
                {"id": second, "content": "Second revised fact"}
            ],
            "retirements": [],
            "creates": [{"content": "A durable combined fact"}]
        })
        .to_string(),
        calls: AtomicU32::new(0),
    });

    let stats = consolidate_plan_fixture(
        Some(memory.clone()),
        Some(llm.clone()),
        &user,
        50,
        &InsightsSection::default(),
    )
    .await
    .unwrap();
    assert_eq!((stats.updated, stats.retired, stats.created), (2, 0, 1));
    assert!(stats.summary.contains("atomically"));
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);

    let stored_first = memory.get(user.user_id, &first).await.unwrap().unwrap();
    let stored_second = memory.get(user.user_id, &second).await.unwrap().unwrap();
    assert_eq!(stored_first.content, "First revised fact");
    assert_eq!(stored_first.revision, 2);
    assert_eq!(stored_second.content, "Second revised fact");
    assert_eq!(stored_second.revision, 2);
    assert_eq!(
        memory
            .list(user.user_id, MemoryListOpts::default())
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        memory
            .write_snapshot(user.user_id)
            .await
            .unwrap()
            .privacy_epoch,
        original_epoch
    );
}

#[tokio::test]
async fn candidate_write_after_clear_returns_error_instead_of_zero_created_success() {
    let memory = MemoryService::new();
    let user = insight_user();
    let context = memory.write_snapshot(user.user_id).await.unwrap();
    memory.clear(user.user_id).await.unwrap();
    let cleared_epoch = memory
        .write_snapshot(user.user_id)
        .await
        .unwrap()
        .privacy_epoch;
    assert!(cleared_epoch > context.privacy_epoch);
    let candidates = parse_memory_extraction_response(
        r#"{"memories":[{"content":"Old source says the user prefers concise explanations","confidence":0.95}]}"#,
    ).unwrap().memories;

    let error = store_memory_candidates(&memory, &user, &context, candidates, "extraction")
        .await
        .expect_err("a rejected old candidate must not be swallowed as Ok(0)");
    let message = error.to_string();
    assert!(
        message.contains("extraction batch was not confirmed"),
        "{message}"
    );
    assert!(
        message.contains("memory privacy epoch changed"),
        "{message}"
    );
    assert!(
        memory
            .list(user.user_id, MemoryListOpts::default())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        memory
            .write_snapshot(user.user_id)
            .await
            .unwrap()
            .privacy_epoch,
        cleared_epoch
    );
}

#[tokio::test]
async fn invalid_later_consolidation_target_leaves_the_entire_plan_unwritten() {
    let memory = Arc::new(MemoryService::new());
    let user = insight_user();
    let id = memory
        .store(
            user.user_id,
            MemoryNode::new("core", "fact", "Original fact", 0.8),
        )
        .await
        .unwrap();
    let llm = Arc::new(FixedMemoryLlm {
        response: serde_json::json!({
            "updates": [{"id": id, "content": "Revised fact"}],
            "retirements": [{"id": Uuid::new_v4(), "status": "superseded", "reason": "duplicate"}],
            "creates": [{"content": "New fact"}]
        })
        .to_string(),
        calls: AtomicU32::new(0),
    });
    assert!(
        consolidate_plan_fixture(
            Some(memory.clone()),
            Some(llm),
            &user,
            50,
            &InsightsSection::default(),
        )
        .await
        .is_err()
    );
    let unchanged = memory.get(user.user_id, &id).await.unwrap().unwrap();
    assert_eq!(
        (unchanged.content.as_str(), unchanged.revision),
        ("Original fact", 1)
    );
    assert_eq!(
        memory
            .list(user.user_id, MemoryListOpts::default())
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn consolidation_retirement_preserves_evidence_and_does_not_advance_privacy_epoch() {
    let memory = Arc::new(MemoryService::new());
    let user = insight_user();
    let id = memory
        .store(
            user.user_id,
            MemoryNode::new("core", "fact", "Old wording", 0.8),
        )
        .await
        .unwrap();
    let epoch = memory
        .write_snapshot(user.user_id)
        .await
        .unwrap()
        .privacy_epoch;
    let llm = Arc::new(FixedMemoryLlm {
        response: serde_json::json!({
            "retirements": [{"id": id, "status": "superseded", "reason": "refined wording"}],
            "creates": [{"content": "Refined wording"}]
        })
        .to_string(),
        calls: AtomicU32::new(0),
    });
    let stats = consolidate_plan_fixture(
        Some(memory.clone()),
        Some(llm),
        &user,
        50,
        &InsightsSection::default(),
    )
    .await
    .unwrap();
    assert_eq!((stats.retired, stats.created), (1, 1));
    let retired = memory.get(user.user_id, &id).await.unwrap().unwrap();
    assert_eq!(retired.status, "superseded");
    assert_eq!(retired.content, "Old wording");
    assert_eq!(
        memory
            .write_snapshot(user.user_id)
            .await
            .unwrap()
            .privacy_epoch,
        epoch
    );
    let mut forget = memory.write_snapshot(user.user_id).await.unwrap();
    forget.source_memories = vec![(id, retired.revision)];
    assert_eq!(
        memory
            .forget_many_guarded(&forget, &forget.source_memories)
            .await
            .unwrap(),
        2
    );
    assert!(
        memory
            .list(user.user_id, MemoryListOpts::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[test]
fn parses_memory_extraction_with_fence_and_reason() {
    let parsed = parse_memory_extraction_response(
        "```json\n{\"memories\":[{\"content\":\"KeQiao likes tsukuyomi\",\"memory_layer\":\"preference\",\"node_type\":\"preference\",\"importance\":0.9,\"confidence\":0.95,\"reason\":\"explicitly stated\"}]}\n```",
    )
    .unwrap();

    assert_eq!(parsed.memories.len(), 1);
    assert_eq!(parsed.memories[0].content, "KeQiao likes tsukuyomi");
    assert_eq!(
        parsed.memories[0].memory_layer.as_deref(),
        Some("preference")
    );
}

#[test]
fn parses_memory_consolidation_with_surrounding_text() {
    let parsed = parse_memory_consolidation_response(
        "Result:\n{\"summary\":\"merged duplicates\",\"updates\":[{\"id\":\"mem_1\",\"content\":\"KeQiao likes Japanese music\",\"importance\":0.8,\"reason\":\"clearer\"}],\"retirements\":[{\"id\":\"mem_2\",\"status\":\"superseded\",\"reason\":\"duplicate\"}],\"creates\":[]}\nThanks.",
    )
    .unwrap();

    assert_eq!(parsed.summary.as_deref(), Some("merged duplicates"));
    assert_eq!(parsed.updates[0].id, "mem_1");
    assert_eq!(parsed.retirements[0].id, "mem_2");
}
