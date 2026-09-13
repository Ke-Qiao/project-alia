use alia_server::modules::memory::context::{ContextPolicy, ContextService, ContextSource};
use futures::FutureExt;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
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
use alia_server::providers::memory::AliaMemory;
use async_trait::async_trait;
use futures::Stream;
use futures::stream;
use uuid::Uuid;

struct StaticLlm {
    response: String,
    gate: Option<(
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    )>,
}

impl StaticLlm {
    fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            gate: None,
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
        if let Some((entered, release)) = &self.gate {
            entered.notify_one();
            release.notified().await;
        }
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

static CAPTURE_DATABASE_CASES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

async fn with_capture_fixture<F, Fut>(test: F)
where
    F: FnOnce(AliaMemory, PendingTextPersistence) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let _slot = CAPTURE_DATABASE_CASES.acquire().await.unwrap();
    let url = std::env::var("ALIA_TEST_DATABASE_URL")
        .expect("memory capture requires isolated real PostgreSQL");
    let admin = sea_orm::Database::connect(&url).await.unwrap();
    let schema = format!("capture_context_{}", Uuid::new_v4().simple());
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
        .await
        .unwrap();
    let mut options = sea_orm::ConnectOptions::new(url);
    options
        .max_connections(4)
        .set_schema_search_path(format!("{schema},public"))
        .sqlx_logging(false);
    let db = sea_orm::Database::connect(options).await.unwrap();
    let outcome = std::panic::AssertUnwindSafe(async {
        alia_server::modules::platform::migrations::run(&db).await.unwrap();
        let owner = Uuid::new_v4();
        let session = Uuid::new_v4();
        let source = Uuid::new_v4();
        let request = Uuid::new_v4();
        let received_at = chrono::Utc::now();
        let query = "I prefer concise answers, but can you summarize this article now?";
        let answer = "Sure, here is a concise summary.";
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,'capture@test.invalid','Capture','not-a-login','user','active')",[owner.into()])).await.unwrap();
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'test-channel',$2)",[session.into(),owner.to_string().into()])).await.unwrap();
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO memory_privacy_state(user_id,privacy_epoch) VALUES($1,0)",[owner.into()])).await.unwrap();
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user',$4,$5,0)",
            vec![source.into(),session.into(),owner.to_string().into(),query.into(),received_at.into()])).await.unwrap();
        let context = ContextService::new(db.clone(), std::sync::Arc::new(alia_server::providers::embedding::NoopEmbedding));
        context.initialize_policy(ContextPolicy::from_config(&memory_config(), &Default::default()).unwrap()).await.unwrap();
        let generation = context.materialize(ContextSource {
            owner_id:owner,request_id:request,channel_id:"test-channel".into(),
            conversation_id:Some(session),source_event_id:Some(source),source_received_at:received_at,source_privacy_epoch:0,
        }, query).await.unwrap();
        let ledger = alia_server::modules::delivery::repository::DeliveryRepository::new(db.clone());
        let response = ledger.prepare(alia_server::modules::delivery::types::NewResponse {
            key:alia_server::modules::delivery::types::ResponseKey {user_id:owner,request_id:request,output_key:"response".into()},
            context_id:generation.lease().id(),conversation_id:Some(session),source_event_id:Some(source),
            source_received_at:received_at,source_privacy_epoch:Some(0),channel_id:"test-channel".into(),
            kind:"assistant_response".into(),content:serde_json::json!({"kind":"assistant_response"}),
        }).await.unwrap();
        let tx = db.begin().await.unwrap();
        let assistant = Uuid::new_v4();
        ledger.mark_generated_in_transaction(&tx,owner,response.response_id,assistant,serde_json::json!({"text":answer})).await.unwrap();
        tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_messages(id,session_id,role,content) VALUES($1,$2,'assistant',$3)",[assistant.into(),session.into(),answer.into()])).await.unwrap();
        tx.commit().await.unwrap();
        let memory = AliaMemory::new();
        memory.attach_runtime_for_test(db.clone(),None,memory_config()).await.unwrap();
        let pending = PendingTextPersistence {
            context:generation.lease().binding(),source_received_at:received_at,source_privacy_epoch:0,source_event_id:Some(source),
            response_id:response.response_id,session_id:session,user_id:owner,user_id_text:owner.to_string(),
            query_text:query.into(),response_text:answer.into(),channel_id:"test-channel".into(),cache_write:None::<CacheWrite>,
        };
        tokio::time::timeout(std::time::Duration::from_secs(15),test(memory,pending)).await.unwrap();
    }).catch_unwind().await;
    db.close().await.unwrap();
    admin
        .execute_unprepared(&format!("DROP SCHEMA {schema} CASCADE"))
        .await
        .unwrap();
    admin.close().await.unwrap();
    if let Err(error) = outcome {
        std::panic::resume_unwind(error);
    }
}

fn memory_config() -> MemorySection {
    MemorySection {
        enabled: true,
        capture_mode: MemoryCaptureMode::LlmSelected,
        capture_timeout_sec: 3,
        capture_min_confidence: 0.65,
        capture_min_importance: 0.45,
        sensitive_filter_enabled: true,
        active_embedding_profile: "openai.text-embedding-3-small.1536.cosine.v1".to_string(),
        graph_depth: 2,
        consolidation_enabled: true,
        paused_until: None,
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
async fn pending_turn_received_before_clear_cannot_be_extracted_later() {
    with_capture_fixture(|memory, pending| async move {
        let user_id = pending.user_id;
        memory.clear(user_id).await.unwrap();
        let llm = StaticLlm::new(
            r#"{"memories":[{"content":"用户喜欢咖啡","importance":0.9,"confidence":0.9}]}"#,
        );
        let trace =
            capture_turn_memory_with_backends(&llm, &memory, &pending, &memory_config()).await;
        assert_eq!(trace.status, MemoryCaptureStatus::Skipped);
        assert_eq!(
            trace.reason.as_deref(),
            Some("source_privacy_epoch_revoked")
        );
        assert!(
            memory
                .list(user_id, MemoryListOpts::default())
                .await
                .unwrap()
                .is_empty()
        );
    })
    .await;
}

#[tokio::test]
async fn deletion_while_extractor_is_waiting_rejects_its_late_result() {
    with_capture_fixture(|memory, pending| async move {
        let user_id = pending.user_id;
        let mut llm = StaticLlm::new(
            r#"{"memories":[{"content":"用户喜欢咖啡","importance":0.9,"confidence":0.9}]}"#,
        );
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        llm.gate = Some((entered.clone(), release.clone()));
        let config = memory_config();
        let capture = capture_turn_memory_with_backends(&llm, &memory, &pending, &config);
        let forget = async {
            entered.notified().await;
            memory.clear(user_id).await.unwrap();
            release.notify_one();
        };
        let (trace, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(capture, forget)
        })
        .await
        .unwrap();
        assert_eq!(trace.stored, 0);
        assert_eq!(trace.skipped, 1);
        assert!(
            memory
                .list(user_id, MemoryListOpts::default())
                .await
                .unwrap()
                .is_empty()
        );
    })
    .await;
}

#[tokio::test]
async fn llm_selected_capture_stores_only_high_confidence_durable_memories() {
    with_capture_fixture(|memory, pending| async move {
        let user_id = pending.user_id;
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

        let trace =
            capture_turn_memory_with_backends(&llm, &memory, &pending, &memory_config()).await;

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
    })
    .await;
}

#[tokio::test]
async fn llm_selected_capture_does_not_store_when_model_returns_no_memories() {
    with_capture_fixture(|memory, pending| async move {
        let user_id = pending.user_id;
        let llm = StaticLlm::new(r#"{"memories":[]}"#);

        let trace =
            capture_turn_memory_with_backends(&llm, &memory, &pending, &memory_config()).await;

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
    })
    .await;
}

#[tokio::test]
async fn llm_selected_capture_blocks_sensitive_memories() {
    with_capture_fixture(|memory, pending| async move {
        let user_id = pending.user_id;
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

        let trace =
            capture_turn_memory_with_backends(&llm, &memory, &pending, &memory_config()).await;

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
    })
    .await;
}

#[tokio::test]
async fn llm_selected_capture_updates_existing_preference_memory() {
    with_capture_fixture(|memory, pending| async move {
        let user_id = pending.user_id;
        memory
            .store(
                user_id,
                alia_server::providers::adapter::MemoryNode::new(
                    "preference",
                    "preference",
                    "The user prefers detailed replies.",
                    0.4,
                )
                .with_metadata(serde_json::json!({"source": "test"}))
                .with_runtime_fields("preference", "user", 0.8, 0.5, "internal"),
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

        let trace =
            capture_turn_memory_with_backends(&llm, &memory, &pending, &memory_config()).await;

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
    })
    .await;
}
