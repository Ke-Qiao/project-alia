//! Real PostgreSQL and runtime-owned Insights; only LLM inference is controlled.
use super::*;
use alia_server::event::ServerEvent;
use alia_server::state::AppState;
use alia_server::{
    modules::{
        memory::{MemoryListOpts, MemoryNode},
        platform::insights::{InsightPhase, InsightTask},
    },
    providers::adapter::ChatMessage,
};
use sea_orm::{DatabaseBackend, Statement};

#[derive(Clone)]
struct ControlledLlm {
    response: Arc<Mutex<String>>,
    requests: Arc<Mutex<Vec<Vec<ChatMessage>>>>,
    calls: Arc<AtomicUsize>,
    entered: Arc<Semaphore>,
    release: Option<Arc<Semaphore>>,
}
impl ControlledLlm {
    fn new(blocked: bool) -> Self {
        Self {
            response: Arc::new(Mutex::new("{}".into())),
            requests: Arc::new(Mutex::new(Vec::new())),
            calls: Arc::new(AtomicUsize::new(0)),
            entered: Arc::new(Semaphore::new(0)),
            release: blocked.then(|| Arc::new(Semaphore::new(0))),
        }
    }
    fn update(&self, id: &str, content: &str) {
        *self.response.lock().unwrap() = serde_json::json!({
            "updates": [{"id": id, "content": content}],
        })
        .to_string();
    }
}
#[async_trait]
impl ModuleLifecycle for ControlledLlm {
    fn name(&self) -> &'static str {
        "insights-test"
    }
}
impl ControlledLlm {
    fn service(&self) -> LlmService {
        let fixture = self.clone();
        test_text("ControlledLlm", move |messages| {
            let fixture = fixture.clone();
            async move {
                fixture.requests.lock().unwrap().push(messages.to_vec());
                fixture.calls.fetch_add(1, Ordering::SeqCst);
                fixture.entered.add_permits(1);
                if let Some(release) = &fixture.release {
                    release.acquire().await.unwrap().forget();
                }
                Ok(fixture.response.lock().unwrap().clone())
            }
        })
    }
}

fn enable_insights(f: &mut Fixture) {
    f.config.policies.memory.enabled = true;
    f.config.policies.insights.enabled = true;
    f.config.policies.insights.extraction_enabled = true;
    f.config.policies.insights.max_concurrent_insights = 1;
    f.config.policies.insights.consolidation_idle_minutes = 525_600;
    // No clock-dependent cron phase should run during these controlled tests.
    f.config.policies.insights.reflection_schedule = "0 0 0 1 1 *".into();
    f.config.policies.insights.habit_detection_schedule = "0 0 0 1 1 *".into();
}

async fn user(state: &AppState) -> Uuid {
    let id = Uuid::new_v4();
    state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Insights test','not-a-password','user','active')",
        vec![id.into(), format!("{id}@test.invalid").into()],
    )).await.unwrap();
    id
}

async fn seed(state: &AppState, owner: Uuid) -> String {
    state
        .modules
        .memory
        .store(
            owner,
            MemoryNode::new("core", "fact", "Original preference", 0.8),
        )
        .await
        .unwrap()
}

async fn disable(state: &AppState, owner: Uuid) {
    state
        .db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE users SET status='disabled' WHERE id=$1",
            [owner.into()],
        ))
        .await
        .unwrap();
}

async fn pending(state: &AppState, owner: Uuid, expected: u32) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if state
                .modules
                .insights
                .status(owner)
                .await
                .unwrap()
                .pending_tasks
                == expected
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn assert_no_completion(events: &mut tokio::sync::broadcast::Receiver<ServerEvent>) {
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, ServerEvent::InsightCompleted { .. }));
    }
}

#[tokio::test]
async fn extraction_uses_current_source_epoch_without_comparing_server_and_database_clocks() {
    with_fixture(|mut f| async move {
        enable_insights(&mut f);
        let llm = ControlledLlm::new(false);
        *llm.response.lock().unwrap() = serde_json::json!({
            "memories": [{"content": "Current source preference", "confidence": 0.9}]
        }).to_string();
        let runtime = f.builder().with_major_llm(llm.service()).build().await.unwrap();
        let state = runtime.state().clone();
        let owner = user(&state).await;
        let foreign_owner = user(&state).await;
        let conversation = &state.modules.conversation;
        let session = conversation.create_session("insights-source", &owner.to_string()).await.unwrap();
        let before_admission = alia_server::modules::memory::source::capture_source_epochs(&state.db, owner).await.unwrap();
        let before = before_admission.privacy_epoch;
        let fast = chrono::Utc::now() + chrono::Duration::hours(1);
        let slow = chrono::Utc::now() - chrono::Duration::hours(1);
        conversation.append_sourced_message(owner, session, ChatMessage::new("user", "OLD_BEFORE_CLEAR"), fast, before, before_admission.relationship_epoch).await.unwrap();
        state.modules.memory.clear(owner).await.unwrap();
        conversation.append_sourced_message(owner, session, ChatMessage::new("user", "OLD_LATE_PERSISTED"), fast, before, before_admission.relationship_epoch).await.unwrap();
        let current_admission = alia_server::modules::memory::source::capture_source_epochs(&state.db, owner).await.unwrap();
        let current = current_admission.privacy_epoch;
        assert_eq!(current, before + 1);
        for (text, received) in [("CURRENT_FAST_CLOCK", fast), ("CURRENT_SLOW_CLOCK", slow)] {
            conversation.append_sourced_message(owner, session, ChatMessage::new("user", text), received, current, current_admission.relationship_epoch).await.unwrap();
        }
        // This deliberate NULL-epoch fixture has a valid timestamp and owner, so
        // the test isolates missing admission provenance from other predicates.
        state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_messages(id,session_id,role,content,user_id,source_received_at) VALUES($1,$2,'user','NULL_EPOCH_SOURCE',$3,$4)",
            vec![Uuid::new_v4().into(),session.into(),owner.to_string().into(),fast.into()],
        )).await.unwrap();
        conversation.append_message(session, ChatMessage::new("assistant", "ASSISTANT_DERIVED_HISTORY")).await.unwrap();
        let foreign_session = conversation.create_session("insights-source", &foreign_owner.to_string()).await.unwrap();
        let foreign_epoch_admission = alia_server::modules::memory::source::capture_source_epochs(&state.db, foreign_owner).await.unwrap();
        let foreign_epoch = foreign_epoch_admission.privacy_epoch;
        conversation.append_sourced_message(foreign_owner, foreign_session, ChatMessage::new("user", "FOREIGN_OWNER_SOURCE"), fast, foreign_epoch, foreign_epoch_admission.relationship_epoch).await.unwrap();

        state.modules.insights.trigger(owner, InsightPhase::Extraction).await.unwrap();
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
        let transcript = llm.requests.lock().unwrap()[0].iter().map(|message| message.content.as_str()).collect::<Vec<_>>().join("\n");
        assert!(transcript.contains("CURRENT_FAST_CLOCK"));
        assert!(transcript.contains("CURRENT_SLOW_CLOCK"));
        for excluded in ["OLD_BEFORE_CLEAR", "OLD_LATE_PERSISTED", "NULL_EPOCH_SOURCE", "ASSISTANT_DERIVED_HISTORY", "FOREIGN_OWNER_SOURCE"] {
            assert!(!transcript.contains(excluded), "unexpected source in LLM input: {excluded}");
        }
        let memories = state.modules.memory.list(owner, MemoryListOpts::default()).await.unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].content, "Current source preference");
        // A memory clear must not discard original user conversation history.
        let history = conversation.get_history(session, 20).await.unwrap();
        assert!(history.iter().any(|message| message.content == "OLD_BEFORE_CLEAR"));
        assert!(history.iter().any(|message| message.content == "OLD_LATE_PERSISTED"));
        runtime.shutdown().await.unwrap();
    }).await;
}

#[tokio::test]
async fn stale_snapshot_reaches_trigger_as_error_and_persists_failure_log() {
    with_fixture(|mut f| async move {
        enable_insights(&mut f);
        let llm = ControlledLlm::new(true);
        let runtime = f
            .builder()
            .with_major_llm(llm.service())
            .build()
            .await
            .unwrap();
        let state = runtime.state().clone();
        let owner = user(&state).await;
        let id = seed(&state, owner).await;
        llm.update(&id, "Late replacement must not survive erasure");
        let mut events = state.event_bus.subscribe();
        let insights = state.modules.insights.clone();
        let request =
            tokio::spawn(async move { insights.trigger(owner, InsightPhase::Consolidation).await });
        signalled(&llm.entered).await;
        state.modules.memory.clear(owner).await.unwrap();
        llm.release.as_ref().unwrap().add_permits(1);
        let error = request.await.unwrap().unwrap_err();
        assert!(
            format!("{error:#}").contains("privacy epoch changed"),
            "{error:#}"
        );
        assert!(
            state
                .modules
                .memory
                .list(owner, MemoryListOpts::default())
                .await
                .unwrap()
                .is_empty()
        );
        let log = state
            .db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT summary FROM insight_logs WHERE user_id=$1",
                [owner.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert!(
            log.try_get::<String>("", "summary")
                .unwrap()
                .contains("Phase failed")
        );
        assert_no_completion(&mut events);
        pending(&state, owner, 0).await;
        runtime.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn log_insert_failure_is_error_even_when_memory_batch_already_committed() {
    with_fixture(|mut f| async move {
        enable_insights(&mut f);
        let llm = ControlledLlm::new(false);
        let runtime = f.builder().with_major_llm(llm.service()).build().await.unwrap();
        let state = runtime.state().clone();
        let owner = user(&state).await;
        let id = seed(&state, owner).await;
        llm.update(&id, "Committed preference");
        state.db.execute_unprepared(
            "CREATE FUNCTION reject_insights_log() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected insights log failure'; END $$;
             CREATE TRIGGER reject_insights_log BEFORE INSERT ON insight_logs FOR EACH ROW EXECUTE FUNCTION reject_insights_log();"
        ).await.unwrap();
        let mut events = state.event_bus.subscribe();
        let error = state.modules.insights.trigger(owner, InsightPhase::Consolidation).await.unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("injected insights log failure"), "{message}");
        assert!(message.contains("may already be committed"), "{message}");
        let stored = state.modules.memory.get(owner, &id).await.unwrap().unwrap();
        assert_eq!(stored.content, "Committed preference");
        assert_eq!(stored.revision, 2);
        let count = state.db.query_one(Statement::from_string(DatabaseBackend::Postgres,
            "SELECT count(*) AS count FROM insight_logs",
        )).await.unwrap().unwrap().try_get::<i64>("", "count").unwrap();
        assert_eq!(count, 0);
        assert_no_completion(&mut events);
        pending(&state, owner, 0).await;
        runtime.shutdown().await.unwrap();
    }).await;
}

#[tokio::test]
async fn disabled_cached_user_is_rejected_by_trigger_and_schedule_without_llm() {
    with_fixture(|mut f| async move {
        enable_insights(&mut f);
        let llm = ControlledLlm::new(false);
        let runtime = f.builder().with_major_llm(llm.service()).build().await.unwrap();
        let state = runtime.state().clone();
        let owner = user(&state).await;
        let id = seed(&state, owner).await;
        let mut events = state.event_bus.subscribe();
        // A completed post-conversation phase proves the listener cached this user.
        // The event is retried only until the listener has subscribed after startup.
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut retry = tokio::time::interval(Duration::from_millis(50));
            loop {
                tokio::select! {
                    _ = retry.tick() => {
                        state.event_bus.publish(ServerEvent::ConversationTurnCompleted {
                            channel_id: "insights-test".into(), user_id: owner.to_string(),
                            user_uuid: owner, session_id: Uuid::new_v4(),
                        });
                        state.event_bus.publish(ServerEvent::VoiceStateChanged { state: "idle".into() });
                    }
                    event = events.recv() => {
                        if matches!(event.unwrap(), ServerEvent::InsightCompleted { phase, .. } if phase == "extraction") {
                            break;
                        }
                    }
                }
            }
        }).await.unwrap();
        disable(&state, owner).await;
        let error = state.modules.insights.trigger(owner, InsightPhase::Consolidation).await.unwrap_err();
        assert!(format!("{error:#}").contains("currently active account"), "{error:#}");
        let error = state.modules.insights.schedule(InsightTask {
            user_id: owner, phase: "consolidation".into(), trigger: "test-schedule".into(),
            context: serde_json::json!({}),
        }).await.unwrap_err();
        assert!(format!("{error:#}").contains("currently active account"), "{error:#}");
        assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
        let stored = state.modules.memory.get(owner, &id).await.unwrap().unwrap();
        assert_eq!(stored.content, "Original preference");
        assert_eq!(stored.revision, 1);
        pending(&state, owner, 0).await;
        runtime.shutdown().await.unwrap();
    }).await;
}

#[tokio::test]
async fn cancelled_trigger_releases_count_and_waiter_rechecks_account_after_capacity() {
    with_fixture(|mut f| async move {
        enable_insights(&mut f);
        let llm = ControlledLlm::new(true);
        let runtime = f
            .builder()
            .with_major_llm(llm.service())
            .build()
            .await
            .unwrap();
        let state = runtime.state().clone();
        let first_owner = user(&state).await;
        let second_owner = user(&state).await;
        seed(&state, first_owner).await;
        seed(&state, second_owner).await;
        let insights = state.modules.insights.clone();
        let first = tokio::spawn(async move {
            insights
                .trigger(first_owner, InsightPhase::Consolidation)
                .await
        });
        signalled(&llm.entered).await;
        let insights = state.modules.insights.clone();
        let waiting = tokio::spawn(async move {
            insights
                .trigger(second_owner, InsightPhase::Consolidation)
                .await
        });
        pending(&state, first_owner, 2).await;
        disable(&state, second_owner).await;
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let error = waiting.await.unwrap().unwrap_err();
        assert!(
            format!("{error:#}").contains("currently active account"),
            "{error:#}"
        );
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
        pending(&state, first_owner, 0).await;
        runtime.shutdown().await.unwrap();
    })
    .await;
}
