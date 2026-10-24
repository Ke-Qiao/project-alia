//! Production ExecutionService and real PostgreSQL. Infrastructure is mandatory.
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::time::Duration;

use alia_server::config::ToolsSection;
use alia_server::modules::execution::repository::ExecutionRepository;
use alia_server::modules::execution::types::{EffectRecord, EffectStatus, ExecutionError};
use alia_server::modules::execution::{ExecutionService, ExecutionToolRegistry};
use alia_server::modules::memory::context::{ContextPolicy, ContextService, ContextSource};
use alia_server::modules::platform::lifecycle::ModuleHealth;
use alia_server::modules::platform::migrations;
use alia_server::providers::adapter::{ToolCall, ToolContext, ToolEntry, ToolRegistry, ToolResult};
use alia_server::tools::BuiltinToolRegistry;
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
    TransactionTrait,
};
use serde_json::json;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

static DATABASE_CASES: Semaphore = Semaphore::const_new(3);

struct Probe {
    confirmation: bool,
    revision: Option<&'static str>,
    permits: Semaphore,
    calls: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
    outcome_mode: AtomicU8,
    sources: std::sync::Mutex<Vec<(chrono::DateTime<chrono::Utc>, Option<Uuid>, Option<i64>)>>,
}

struct Running<'a>(&'a AtomicUsize);
impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Probe {
    fn entry(self: &Arc<Self>) -> anyhow::Result<Arc<ToolEntry>> {
        let owner = self.clone();
        let dynamic = rig_core::tool::DynamicTool::new_with_context(
            rig_core::message::ToolName::new("execution_probe")?,
            "Execution test probe",
            json!({"type":"object"}),
            move |context, args| {
                let owner = owner.clone();
                Box::pin(async move {
                    let invocation = context
                        .require::<ToolContext>()
                        .map_err(alia_server::tools::entry::tool_error)?;
                    owner
                        .execute(args, &invocation)
                        .await
                        .map_err(alia_server::tools::entry::tool_error)?
                        .publish(context)
                })
            },
        );
        let owner = self.clone();
        Ok(ToolEntry::dynamic(
            dynamic,
            "builtin",
            self.revision.unwrap_or(""),
            self.confirmation,
            None,
            json!({}),
        )?
        .with_definitive_rejection(move |_| owner.outcome_mode.load(Ordering::SeqCst) == 3))
    }
    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        self.sources.lock().unwrap().push((
            context.source_received_at,
            context.source_event_id,
            context.source_privacy_epoch,
        ));
        self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        let _running = Running(&self.active);
        self.permits.acquire().await.unwrap().forget();
        let mode = self.outcome_mode.load(Ordering::SeqCst);
        anyhow::ensure!(mode != 2, "transport disconnected after dispatch");
        Ok(ToolResult {
            presentation: None,
            call_id: String::new(),
            success: mode == 0,
            output: arguments,
            display: None,
        })
    }
}

#[derive(Clone)]
struct Fixture {
    db: DatabaseConnection,
    admin: DatabaseConnection,
    schema: String,
    user: Uuid,
    service: Arc<ExecutionService>,
    probe: Arc<Probe>,
    registry: Arc<ExecutionToolRegistry>,
    raw: Arc<BuiltinToolRegistry>,
}

impl Fixture {
    async fn new(confirmation: bool, timeout: u64, permits: usize) -> Self {
        let url = std::env::var("ALIA_TEST_DATABASE_URL")
            .expect("set ALIA_TEST_DATABASE_URL to disposable PostgreSQL");
        let mut options = ConnectOptions::new(url.clone());
        options
            .max_connections(1)
            .connect_timeout(Duration::from_secs(5))
            .sqlx_logging(false);
        let admin = Database::connect(options)
            .await
            .expect("test PostgreSQL must be available");
        let schema = format!("execution_service_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut options = ConnectOptions::new(url);
        options
            .max_connections(6)
            .connect_timeout(Duration::from_secs(5))
            .set_schema_search_path(format!("{schema},public"))
            .sqlx_logging(false);
        let db = Database::connect(options).await.unwrap();
        migrations::run(&db)
            .await
            .expect("fresh production migrations");
        let user = Uuid::new_v4();
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Probe','disabled','user','active')",
            vec![user.into(), format!("{user}@service.test").into()])).await.unwrap();
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO tools(name,description,parameters,source,require_confirmation,enabled) VALUES('execution_probe','Probe',$1,'builtin',$2,true)",
            vec![json!({"type":"object"}).into(), confirmation.into()])).await.unwrap();
        let probe = Arc::new(Probe {
            confirmation,
            revision: Some("execution.test.Probe"),
            permits: Semaphore::new(permits),
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            outcome_mode: AtomicU8::new(0),
            sources: std::sync::Mutex::new(Vec::new()),
        });
        let raw = Arc::new(BuiltinToolRegistry::new());
        raw.register_executor(probe.entry().unwrap());
        let service = ExecutionService::new(
            ExecutionRepository::new(db.clone()),
            raw.clone(),
            &ToolsSection {
                tool_call_timeout_sec: timeout,
                ..ToolsSection::default()
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let registry = Arc::new(ExecutionToolRegistry::new(service.clone()));
        Self {
            db,
            admin,
            schema,
            user,
            service,
            probe,
            registry,
            raw,
        }
    }
    fn contexts(&self) -> ContextService {
        ContextService::new(
            self.db.clone(),
            Arc::new(alia_server::providers::embedding::NoopEmbedding),
        )
    }
    async fn bind_source(&self, context: &mut ToolContext) {
        if context.context_id.is_some() {
            context.request_id = Uuid::new_v4();
        }
        let service = self.contexts();
        service
            .initialize_policy(
                ContextPolicy::from_config(&Default::default(), &Default::default()).unwrap(),
            )
            .await
            .unwrap();
        let row = self
            .db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT content FROM chat_messages WHERE id=$1",
                [context.source_event_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        let raw: String = row.try_get("", "content").unwrap();
        let generation = service
            .materialize(
                ContextSource {
                    owner_id: context.user_id,
                    request_id: context.request_id,
                    channel_id: context.channel_id.clone().unwrap(),
                    conversation_id: context.conversation_id,
                    source_event_id: context.source_event_id,
                    source_received_at: context.source_received_at,
                    source_privacy_epoch: context.source_privacy_epoch.unwrap(),
                },
                &raw,
            )
            .await
            .unwrap();
        context.context_id = Some(generation.lease().id());
    }
    async fn context(&self) -> ToolContext {
        let session = Uuid::new_v4();
        let source = Uuid::new_v4();
        let epoch = alia_server::modules::memory::source::capture_source_epoch(&self.db, self.user)
            .await
            .unwrap();
        let mut context = ToolContext {
            context_id: None,
            source_received_at: chrono::Utc::now(),
            source_event_id: Some(source),
            source_privacy_epoch: Some(epoch),
            user_id: self.user,
            request_id: Uuid::new_v4(),
            invocation_key: "0:0".into(),
            conversation_id: Some(session),
            source: "ai".into(),
            channel_id: Some("probe".into()),
            cancel: None,
        };
        self.db
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'probe',$2)",
                vec![session.into(), self.user.to_string().into()],
            ))
            .await
            .unwrap();
        self.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user','Please remember my preference.',$4,$5)",
            vec![source.into(),session.into(),self.user.to_string().into(),context.source_received_at.into(),epoch.into()])).await.unwrap();
        self.bind_source(&mut context).await;
        context
    }
    fn call(&self) -> ToolCall {
        ToolCall {
            tool_name: "execution_probe".into(),
            arguments: json!({"confirmed":true}),
            call_id: "call_0".into(),
        }
    }
    async fn status(&self, id: Uuid, expected: EffectStatus) -> EffectRecord {
        let mut observed = None;
        let result = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let effect = self.service.get(self.user, id).await.unwrap();
                if effect.status == expected {
                    break effect;
                }
                assert!(
                    !effect.status.is_terminal(),
                    "effect {id} reached unexpected terminal status {:?}, expected {expected:?}; completion={:?}",
                    effect.status, effect.completion,
                );
                observed = Some((effect.status, effect.completion));
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        result.unwrap_or_else(|error| {
            panic!(
                "effect {id} did not reach {expected:?}: {error}; last persisted state={observed:?}"
            )
        })
    }
    async fn calls(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.probe.calls.load(Ordering::SeqCst) != expected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("expected executor calls");
    }
    async fn cleanup(self) {
        self.probe.permits.add_permits(32);
        let _ = self.service.shutdown().await;
        self.db.close().await.unwrap();
        self.admin
            .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .await
            .unwrap();
        self.admin.close().await.unwrap();
    }
}

async fn with_fixture<F, Fut>(confirmation: bool, timeout: u64, permits: usize, test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    let _slot = DATABASE_CASES.acquire().await.unwrap();
    let fixture = Fixture::new(confirmation, timeout, permits).await;
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(30),
        test(fixture.clone()),
    ))
    .catch_unwind()
    .await;
    fixture.cleanup().await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("execution service test exceeded deadline"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test]
async fn duplicate_request_and_changed_model_correlation_execute_once() {
    with_fixture(false, 5, 2, |f| async move {
        f.service.start().await.unwrap();
        let context = f.context().await;
        let first = f.service.execute(f.call(), &context).await.unwrap();
        assert!(first.success);
        let mut retry = f.call();
        retry.call_id = "new_model_correlation".into();
        let result = f.service.execute(retry, &context).await.unwrap();
        assert!(result.success);
        assert_eq!(result.call_id, "new_model_correlation");
        assert_eq!(result.output["effect_id"], first.output["effect_id"]);
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
        let next_turn = f
            .service
            .execute(f.call(), &f.context().await)
            .await
            .unwrap();
        assert!(next_turn.success);
        assert_ne!(next_turn.output["effect_id"], first.output["effect_id"]);
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 2);
    })
    .await;
}

#[tokio::test]
async fn policy_change_blocks_old_approval_and_claim_but_fresh_context_can_execute() {
    with_fixture(true, 5, 1, |f| async move {
        let old = f.context().await;
        let proposed = f.service.submit(f.call(), &old).await.unwrap();
        let mut queued_context = old.clone();
        queued_context.invocation_key = "0:1".into();
        let queued = f.service.submit(f.call(), &queued_context).await.unwrap();
        f.service
            .confirm(f.user, queued.effect_id, queued.intent_digest)
            .await
            .unwrap();

        let contexts = f.contexts();
        let store = contexts.policy_store();
        let current = store.current().await.unwrap();
        let mut next = current.policy;
        next.max_history_messages = 7;
        store.replace(current.version, next).await.unwrap();
        assert!(matches!(
            f.service
                .confirm(f.user, proposed.effect_id, proposed.intent_digest)
                .await,
            Err(ExecutionError::Unauthorized(_))
        ));
        assert!(f.service.submit(f.call(), &old).await.is_err());
        f.service.start().await.unwrap();
        let stopped = f.status(queued.effect_id, EffectStatus::Failed).await;
        assert!(stopped.claim_id.is_none());
        assert_eq!(
            serde_json::to_value(stopped.completion).unwrap()["evidence"]["execution_started"],
            false
        );
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);

        let fresh = f
            .service
            .submit(f.call(), &f.context().await)
            .await
            .unwrap();
        f.service
            .confirm(f.user, fresh.effect_id, fresh.intent_digest)
            .await
            .unwrap();
        f.status(fresh.effect_id, EffectStatus::Succeeded).await;
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn revocation_after_dispatch_preserves_the_actual_late_completion() {
    with_fixture(false, 5, 0, |f| async move {
        let old = f.context().await;
        f.service.start().await.unwrap();
        let effect = f.service.submit(f.call(), &old).await.unwrap();
        f.calls(1).await;
        let contexts = f.contexts();
        let store = contexts.policy_store();
        let current = store.current().await.unwrap();
        let mut next = current.policy;
        next.memory_enabled = !next.memory_enabled;
        store.replace(current.version, next).await.unwrap();
        assert!(f.service.submit(f.call(), &old).await.is_err());
        f.probe.permits.add_permits(1);
        let completed = f.status(effect.effect_id, EffectStatus::Succeeded).await;
        assert!(completed.claim_id.is_some());
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn source_notice_cannot_authorize_a_tool_effect() {
    with_fixture(false, 5, 1, |f| async move {
        let mut context = f.context().await;
        let notice = f
            .contexts()
            .source_notice(ContextSource {
                owner_id: f.user,
                request_id: context.request_id,
                channel_id: context.channel_id.clone().unwrap(),
                conversation_id: context.conversation_id,
                source_event_id: context.source_event_id,
                source_received_at: context.source_received_at,
                source_privacy_epoch: context.source_privacy_epoch.unwrap(),
            })
            .await
            .unwrap();
        context.context_id = Some(notice.id());
        assert!(matches!(
            f.service.submit(f.call(), &context).await,
            Err(ExecutionError::Unauthorized(_))
        ));
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
        assert!(
            ExecutionRepository::new(f.db.clone())
                .pending(10)
                .await
                .unwrap()
                .is_empty()
        );
    })
    .await;
}

#[tokio::test]
async fn queued_effect_and_retransmission_preserve_the_original_source() {
    with_fixture(false, 5, 0, |f| async move {
        let mut context = f.context().await;
        context.source_received_at -= chrono::Duration::minutes(5);
        context.source_privacy_epoch = Some(17);
        context.source_event_id = Some(Uuid::new_v4());
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE memory_privacy_state SET privacy_epoch=17 WHERE user_id=$1", [f.user.into()])).await.unwrap();
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user','Original source',$4,17)",
            vec![context.source_event_id.into(),context.conversation_id.into(),f.user.to_string().into(),context.source_received_at.into()])).await.unwrap();
        f.bind_source(&mut context).await;
        let first = f.service.submit(f.call(), &context).await.unwrap();
        assert_eq!(first.status, EffectStatus::Approved);
        assert_eq!(
            first.source_received_at.timestamp_micros(),
            context.source_received_at.timestamp_micros()
        );

        let mut retry_context = context.clone();
        retry_context.source_received_at = chrono::Utc::now();
        let mut retry_call = f.call();
        retry_call.call_id = "another_provider_id".into();
        assert!(f.service.submit(retry_call.clone(), &retry_context).await.is_err(), "a retry cannot replace the source observation attached to its context");
        let retry = f.service.submit(retry_call, &context).await.unwrap();
        assert_eq!(retry.effect_id, first.effect_id);
        assert_eq!(retry.source_received_at, first.source_received_at);
        assert_eq!(retry.source_event_id, first.source_event_id);
        assert_eq!(retry.source_privacy_epoch, Some(17));
        for changed_stamp in [None, Some(18)] {
            let mut changed = context.clone();
            changed.source_privacy_epoch = changed_stamp;
            assert!(matches!(
                f.service.submit(f.call(), &changed).await,
                Err(ExecutionError::IdempotencyConflict | ExecutionError::Unauthorized(_))
            ));
        }
        f.service.start().await.unwrap();
        f.calls(1).await;
        assert_eq!(
            f.probe.sources.lock().unwrap().as_slice(),
            &[(
                first.source_received_at,
                context.source_event_id,
                context.source_privacy_epoch
            )]
        );
        f.probe.permits.add_permits(1);
        f.status(first.effect_id, EffectStatus::Succeeded).await;
    })
    .await;
}

#[tokio::test]
async fn queued_ai_memory_tool_cannot_write_back_after_clear() {
    queued_memory_after_clear_case(chrono::Duration::zero()).await;
}

#[tokio::test]
async fn queued_ai_memory_tool_cannot_survive_clear_when_server_clock_is_ahead() {
    queued_memory_after_clear_case(chrono::Duration::minutes(1)).await;
}

#[tokio::test]
async fn queued_ai_memory_tool_cannot_survive_clear_when_server_clock_is_behind() {
    queued_memory_after_clear_case(-chrono::Duration::minutes(1)).await;
}

async fn queued_memory_after_clear_case(source_clock_offset: chrono::Duration) {
    use alia_server::modules::memory::MemoryService;
    use alia_server::tools::builtin::MemoryStore;

    with_fixture(false, 5, 0, move |f| async move {
        let memory = Arc::new(MemoryService::new());
        memory.attach_runtime_for_test(f.db.clone(), None, Default::default()).await.unwrap();
        let memory_tool = MemoryStore::new(Some(memory.clone())).entry().unwrap();
        let definition = memory_tool.definition();
        // This fixture intentionally leaves the raw registry dependencies unbound;
        // create the same database catalog row that normal builder init seeds.
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO tools(name,description,parameters,source,require_confirmation,enabled) VALUES($1,$2,$3,'builtin',false,true)",
            vec![definition.name.to_string().into(),definition.description.clone().into(),definition.parameters.clone().into()])).await.unwrap();
        f.registry.register_tool_executor("builtin", memory_tool).await.unwrap();
        let session = Uuid::new_v4();
        let source = Uuid::new_v4();
        let mut context = f.context().await;
        context.source_received_at += source_clock_offset;
        context.source_event_id = Some(source);
        context.conversation_id = Some(session);
        context.source_privacy_epoch = Some(alia_server::modules::memory::source::capture_source_epoch(&f.db, f.user).await.unwrap());
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'probe',$2)",
            vec![session.into(),f.user.to_string().into()])).await.unwrap();
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user','Please remember my preference.',$4,$5)",
            vec![source.into(),session.into(),f.user.to_string().into(),context.source_received_at.into(),context.source_privacy_epoch.into()])).await.unwrap();
        f.bind_source(&mut context).await;
        let call = ToolCall { tool_name: "memory_store".into(), arguments: json!({"content":"The user prefers concise replies."}), call_id: "memory_call".into() };
        let effect = f.service.submit(call.clone(), &context).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Approved);
        assert_eq!(effect.source_privacy_epoch, context.source_privacy_epoch);
        memory.clear(f.user).await.unwrap();
        let boundary = memory.write_snapshot(f.user).await.unwrap();
        assert!(boundary.privacy_epoch > context.source_privacy_epoch.unwrap());
        let source_row = f.db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT created_at FROM chat_messages WHERE id=$1", [source.into()],
        )).await.unwrap().unwrap();
        let source_created_at: chrono::DateTime<chrono::Utc> = source_row.try_get("", "created_at").unwrap();
        assert!(source_created_at <= boundary.source_cutoff_at, "source was durably present before clear");
        if source_clock_offset > chrono::Duration::zero() {
            assert!(effect.source_received_at > boundary.source_cutoff_at, "the test must exercise distinct clock domains");
        }
        // Revocation stops the queued effect before the native executor runs.
        context.source_received_at = chrono::Utc::now();
        assert!(f.service.submit(call.clone(), &context).await.is_err());
        f.service.start().await.unwrap();
        let stopped = f.status(effect.effect_id, EffectStatus::Failed).await;
        assert!(stopped.claim_id.is_none());
        assert_eq!(serde_json::to_value(stopped.completion).unwrap()["evidence"]["execution_started"], false);
        assert!(memory.list(f.user, Default::default()).await.unwrap().is_empty());

        // A new, admitted source after clear is valid in every clock domain.
        // In particular a slow Server clock cannot make fresh input look stale.
        let mut fresh = context.clone();
        fresh.request_id = Uuid::new_v4();
        fresh.source_event_id = Some(Uuid::new_v4());
        fresh.source_received_at = chrono::Utc::now() + source_clock_offset;
        fresh.source_privacy_epoch = Some(alia_server::modules::memory::source::capture_source_epoch(&f.db, f.user).await.unwrap());
        assert!(fresh.source_privacy_epoch.unwrap() > effect.source_privacy_epoch.unwrap());
        if source_clock_offset < chrono::Duration::zero() {
            assert!(fresh.source_received_at < boundary.source_cutoff_at);
        }
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user','A new input after clear.',$4,$5)",
            vec![fresh.source_event_id.into(),session.into(),f.user.to_string().into(),fresh.source_received_at.into(),fresh.source_privacy_epoch.into()])).await.unwrap();
        f.bind_source(&mut fresh).await;
        let new_effect = f.service.submit(call, &fresh).await.unwrap();
        f.status(new_effect.effect_id, EffectStatus::Succeeded).await;
        assert_eq!(memory.list(f.user, Default::default()).await.unwrap().len(), 1);
    }).await;
}

#[tokio::test]
async fn ai_memory_tool_validates_persisted_source_owner_and_epoch_in_postgres() {
    use alia_server::modules::memory::MemoryService;
    use alia_server::tools::builtin::MemoryStore;

    with_fixture(false, 5, 0, |f| async move {
        let memory = Arc::new(MemoryService::new());
        memory.attach_runtime_for_test(f.db.clone(), None, Default::default()).await.unwrap();
        let store = MemoryStore::new(Some(memory.clone()));
        let session = Uuid::new_v4();
        let source = Uuid::new_v4();
        let mut context = f.context().await;
        context.source_event_id = Some(source);
        context.conversation_id = Some(session);
        context.source_privacy_epoch = Some(alia_server::modules::memory::source::capture_source_epoch(&f.db, f.user).await.unwrap());
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'probe',$2)",
            vec![session.into(),f.user.to_string().into()])).await.unwrap();
        let arguments = json!({"content":"The user prefers concise replies."});
        assert!(store.execute(serde_json::from_value(arguments.clone()).unwrap(), &context).await.is_err(), "unpersisted source cannot write");
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user','Please remember my preference.',$4,$5)",
            vec![source.into(),session.into(),f.user.to_string().into(),context.source_received_at.into(),context.source_privacy_epoch.into()])).await.unwrap();
        f.bind_source(&mut context).await;
        for epoch in [None, Some(context.source_privacy_epoch.unwrap() + 1)] {
            let unstamped = ToolContext { source_privacy_epoch: epoch, ..context.clone() };
            assert!(store.execute(serde_json::from_value(arguments.clone()).unwrap(), &unstamped).await.is_err(), "missing or different source admission cannot write");
        }
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE chat_sessions SET creator_id=$1 WHERE id=$2", vec![Uuid::new_v4().to_string().into(),session.into()])).await.unwrap();
        assert!(store.execute(serde_json::from_value(arguments.clone()).unwrap(), &context).await.is_err(), "other owner's source cannot write");
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE chat_sessions SET creator_id=$1 WHERE id=$2", vec![f.user.to_string().into(),session.into()])).await.unwrap();
        assert!(store.execute(serde_json::from_value(arguments).unwrap(), &context).await.unwrap().success);
        let saved = memory.list(f.user, Default::default()).await.unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].source_event_ids, vec![source]);
    }).await;
}

#[tokio::test]
async fn model_confirmed_argument_does_not_approve_and_rest_owner_must_match() {
    with_fixture(true, 5, 1, |f| async move {
        f.service.start().await.unwrap();
        let effect = f
            .service
            .submit(f.call(), &f.context().await)
            .await
            .unwrap();
        assert_eq!(effect.status, EffectStatus::Proposed);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
        assert!(
            f.service
                .confirm(
                    Uuid::new_v4(),
                    effect.effect_id,
                    effect.intent_digest.clone()
                )
                .await
                .is_err()
        );
        f.service
            .confirm(f.user, effect.effect_id, effect.intent_digest)
            .await
            .unwrap();
        f.status(effect.effect_id, EffectStatus::Succeeded).await;
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn workers_are_bounded_and_cancelled_queued_voice_never_dispatches() {
    with_fixture(false, 5, 0, |f| async move {
        let mut ids = Vec::new();
        for _ in 0..4 {
            ids.push(
                f.service
                    .submit(f.call(), &f.context().await)
                    .await
                    .unwrap()
                    .effect_id,
            );
        }
        f.service.start().await.unwrap();
        f.calls(4).await;
        let token = CancellationToken::new();
        let mut context = f.context().await;
        context.cancel = Some(token.clone());
        let queued = f.service.submit(f.call(), &context).await.unwrap();
        token.cancel();
        f.probe.permits.add_permits(4);
        f.status(queued.effect_id, EffectStatus::Cancelled).await;
        for id in ids {
            f.status(id, EffectStatus::Succeeded).await;
        }
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 4);
        assert_eq!(f.probe.max_active.load(Ordering::SeqCst), 4);
    })
    .await;
}

#[tokio::test]
async fn shutdown_joins_dispatched_deadline_and_persists_unknown_without_retry() {
    with_fixture(false, 1, 0, |f| async move {
        f.service.start().await.unwrap();
        let effect = f
            .service
            .submit(f.call(), &f.context().await)
            .await
            .unwrap();
        f.calls(1).await;
        let service = f.service.clone();
        let mut stopping = tokio::spawn(async move { service.shutdown().await });
        let _ = f.status(effect.effect_id, EffectStatus::Unknown).await;
        assert_eq!(f.probe.active.load(Ordering::SeqCst), 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut stopping)
                .await
                .is_err()
        );
        f.probe.permits.add_permits(1);
        stopping.await.unwrap().unwrap();
        let effect = f.status(effect.effect_id, EffectStatus::Unknown).await;
        assert!(effect.completion.is_some());
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
        assert_eq!(f.probe.active.load(Ordering::SeqCst), 0);
        let repository = ExecutionRepository::new(f.db.clone());
        assert!(repository.pending(10).await.unwrap().is_empty());
        assert_eq!(repository.recover_expired(10).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
async fn missing_identity_and_revoked_owner_fail_before_executor() {
    with_fixture(false, 5, 1, |f| async move {
        let mut anonymous = f.context().await;
        anonymous.user_id = Uuid::new_v4();
        assert!(matches!(
            f.service.submit(f.call(), &anonymous).await,
            Err(ExecutionError::IdentityUnlinked | ExecutionError::Unauthorized(_))
        ));
        let effect = f
            .service
            .submit(f.call(), &f.context().await)
            .await
            .unwrap();
        f.db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE users SET status='disabled' WHERE id=$1",
            vec![f.user.into()],
        ))
        .await
        .unwrap();
        f.service.start().await.unwrap();
        let stopped = f.status(effect.effect_id, EffectStatus::Failed).await;
        assert!(stopped.claim_id.is_none());
        assert_eq!(
            serde_json::to_value(stopped.completion).unwrap()["evidence"]["execution_started"],
            false
        );
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
    })
    .await;
}

#[tokio::test]
async fn receipt_write_failure_is_shutdown_failure_then_recovers_unknown() {
    with_fixture(false, 5, 1, |f| async move {
        let effect = f.service.submit(f.call(), &f.context().await).await.unwrap();
        f.db.execute_unprepared("CREATE FUNCTION reject_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.status='succeeded' THEN RAISE EXCEPTION 'injected receipt write failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_receipt BEFORE UPDATE ON execution_effects FOR EACH ROW EXECUTE FUNCTION reject_receipt();").await.unwrap();
        f.service.start().await.unwrap();
        f.calls(1).await;
        assert!(f.service.shutdown().await.is_err());
        assert!(matches!(f.service.health(), ModuleHealth::Unhealthy(_)));
        assert_eq!(f.service.get(f.user, effect.effect_id).await.unwrap().status, EffectStatus::Dispatched);
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE execution_effects SET deadline_at=now()-interval '20 seconds' WHERE effect_id=$1", vec![effect.effect_id.into()])).await.unwrap();
        assert_eq!(ExecutionRepository::new(f.db.clone()).recover_expired(10).await.unwrap(), 1);
        f.status(effect.effect_id, EffectStatus::Unknown).await;
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
    }).await;
}

#[tokio::test]
async fn transport_errors_and_legacy_false_results_require_definitive_failure_evidence() {
    with_fixture(false, 5, 3, |f| async move {
        f.service.start().await.unwrap();
        for (mode, expected) in [(1, "unknown"), (2, "unknown"), (3, "failed")] {
            f.probe.outcome_mode.store(mode, Ordering::SeqCst);
            let result = f
                .service
                .execute(f.call(), &f.context().await)
                .await
                .unwrap();
            assert!(!result.success);
            assert_eq!(result.output["execution_status"], expected);
        }
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 3);
    })
    .await;
}

#[tokio::test]
async fn claim_uses_pinned_executor_when_catalog_is_hot_replaced() {
    with_fixture(false, 5, 0, |f| async move {
        let replacement = Arc::new(Probe {
            confirmation: false,
            revision: Some("execution.test.Probe"),
            permits: Semaphore::new(1),
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            outcome_mode: AtomicU8::new(0),
            sources: std::sync::Mutex::new(Vec::new()),
        });
        f.service.start().await.unwrap();
        let first = f
            .service
            .submit(f.call(), &f.context().await)
            .await
            .unwrap();
        f.calls(1).await; // The first effect has committed its claim and entered this Arc.
        f.registry
            .register_tool_executor("builtin", replacement.entry().unwrap())
            .await
            .unwrap();
        f.probe.permits.add_permits(1);
        f.status(first.effect_id, EffectStatus::Succeeded).await;
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
        assert_eq!(replacement.calls.load(Ordering::SeqCst), 0);
        assert!(
            f.service
                .execute(f.call(), &f.context().await)
                .await
                .unwrap()
                .success
        );
        assert_eq!(replacement.calls.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn same_schema_hot_replacement_invalidates_undispatched_approval() {
    with_fixture(false, 5, 1, |f| async move {
        let first = f
            .service
            .submit(f.call(), &f.context().await)
            .await
            .unwrap();
        let replacement = Arc::new(Probe {
            confirmation: false,
            revision: Some("execution.test.Probe"),
            permits: Semaphore::new(1),
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            outcome_mode: AtomicU8::new(0),
            sources: std::sync::Mutex::new(Vec::new()),
        });
        f.registry
            .register_tool_executor("builtin", replacement.entry().unwrap())
            .await
            .unwrap();
        f.service.start().await.unwrap();
        f.status(first.effect_id, EffectStatus::Failed).await;
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
        assert_eq!(replacement.calls.load(Ordering::SeqCst), 0);
        assert!(
            f.service
                .execute(f.call(), &f.context().await)
                .await
                .unwrap()
                .success
        );
        assert_eq!(replacement.calls.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn missing_implementation_revision_is_rejected_at_registration_and_proposal() {
    with_fixture(false, 5, 1, |f| async move {
        for revision in [None, Some("  \t")] {
            let unversioned = Arc::new(Probe {
                confirmation: false,
                revision,
                permits: Semaphore::new(1),
                calls: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
                outcome_mode: AtomicU8::new(0),
                sources: std::sync::Mutex::new(Vec::new()),
            });
            assert!(unversioned.entry().is_err());
            assert_eq!(unversioned.calls.load(Ordering::SeqCst), 0);
        }
        assert!(
            f.service
                .list(f.user, None, 100, None)
                .await
                .unwrap()
                .is_empty()
        );
    })
    .await;
}

#[tokio::test]
async fn replay_of_unknown_effect_returns_existing_fact_without_retrying_executor() {
    with_fixture(false, 5, 2, |f| async move {
        f.probe.outcome_mode.store(2, Ordering::SeqCst);
        f.service.start().await.unwrap();
        let context = f.context().await;
        let first = f.service.execute(f.call(), &context).await.unwrap();
        assert_eq!(first.output["execution_status"], "unknown");
        let retry = f.service.execute(f.call(), &context).await.unwrap();
        assert_eq!(retry.output["effect_id"], first.output["effect_id"]);
        assert_eq!(retry.output["execution_status"], "unknown");
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
        assert!(
            ExecutionRepository::new(f.db.clone())
                .pending(10)
                .await
                .unwrap()
                .is_empty()
        );
    })
    .await;
}

#[tokio::test]
async fn shutdown_cancels_session_intent_without_requiring_prior_parent_cancellation() {
    with_fixture(false, 5, 1, |f| async move {
        let token = CancellationToken::new();
        let mut context = f.context().await;
        context.cancel = Some(token.clone());
        let session_effect = f.service.submit(f.call(), &context).await.unwrap();
        let durable_effect = f
            .service
            .submit(f.call(), &f.context().await)
            .await
            .unwrap();
        assert!(!token.is_cancelled());
        f.service.shutdown().await.unwrap();
        assert!(token.is_cancelled());
        f.status(session_effect.effect_id, EffectStatus::Cancelled)
            .await;
        assert_eq!(
            f.service
                .get(f.user, durable_effect.effect_id)
                .await
                .unwrap()
                .status,
            EffectStatus::Approved
        );
        let pending = ExecutionRepository::new(f.db.clone())
            .pending(10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].effect_id, durable_effect.effect_id);
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
    })
    .await;
}

#[tokio::test]
async fn shutdown_cancellation_write_failure_is_visible_and_does_not_claim_execution() {
    with_fixture(false, 5, 1, |f| async move {
        let token = CancellationToken::new();
        let mut context = f.context().await; context.cancel = Some(token.clone());
        let effect = f.service.submit(f.call(), &context).await.unwrap();
        f.db.execute_unprepared("CREATE FUNCTION reject_cancel() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.status='cancelled' THEN RAISE EXCEPTION 'injected cancellation write failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_cancel BEFORE UPDATE ON execution_effects FOR EACH ROW EXECUTE FUNCTION reject_cancel();").await.unwrap();
        assert!(f.service.shutdown().await.is_err());
        assert!(token.is_cancelled());
        assert!(matches!(f.service.health(), ModuleHealth::Unhealthy(_)));
        assert_eq!(f.service.get(f.user, effect.effect_id).await.unwrap().status, EffectStatus::Approved);
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
    }).await;
}

#[tokio::test]
async fn dropping_shutdown_wait_retains_worker_join_and_receipt_persistence() {
    with_fixture(false, 5, 0, |f| async move {
        f.service.start().await.unwrap();
        let effect = f
            .service
            .submit(f.call(), &f.context().await)
            .await
            .unwrap();
        f.calls(1).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(30), f.service.shutdown())
                .await
                .is_err()
        );
        f.probe.permits.add_permits(1);
        f.service.shutdown().await.unwrap();
        f.status(effect.effect_id, EffectStatus::Succeeded).await;
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
        assert_eq!(f.probe.active.load(Ordering::SeqCst), 0);
    })
    .await;
}

#[tokio::test]
async fn shutdown_waits_for_inflight_admission_before_cancelling_session_intent() {
    with_fixture(false, 5, 1, |f| async move {
        let blocked = f.db.begin().await.unwrap();
        blocked.execute_unprepared("LOCK TABLE execution_effects IN ACCESS EXCLUSIVE MODE").await.unwrap();
        let token = CancellationToken::new();
        let mut context = f.context().await; context.cancel = Some(token.clone());
        let service = f.service.clone(); let call = f.call();
        let proposal = tokio::spawn(async move { service.submit(call, &context).await });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let row = f.admin.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                    "SELECT COUNT(*)::bigint AS waiting FROM pg_locks WHERE relation=$1::regclass AND NOT granted",
                    vec![format!("{}.execution_effects", f.schema).into()])).await.unwrap().unwrap();
                if row.try_get::<i64>("", "waiting").unwrap() > 0 { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        let service = f.service.clone();
        let shutdown = tokio::spawn(async move { service.shutdown().await });
        tokio::task::yield_now().await;
        assert!(!shutdown.is_finished());
        blocked.commit().await.unwrap();
        let effect = proposal.await.unwrap().unwrap();
        shutdown.await.unwrap().unwrap();
        assert!(token.is_cancelled());
        f.status(effect.effect_id, EffectStatus::Cancelled).await;
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
    }).await;
}

#[tokio::test]
async fn shutdown_preserves_unapproved_proposal_for_later_explicit_owner_review() {
    with_fixture(true, 5, 1, |f| async move {
        let mut context = f.context().await;
        context.cancel = Some(CancellationToken::new());
        let proposal = f.service.submit(f.call(), &context).await.unwrap();
        f.service.shutdown().await.unwrap();
        let proposal = f.service.get(f.user, proposal.effect_id).await.unwrap();
        assert_eq!(proposal.status, EffectStatus::Proposed);
        assert!(proposal.requires_confirmation);
        assert!(
            ExecutionRepository::new(f.db.clone())
                .pending(10)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
    })
    .await;
}

#[tokio::test]
async fn locked_ledger_bounds_admission_and_entire_shutdown_cancellation_batch() {
    with_fixture(false, 5, 1, |f| async move {
        let mut effects = Vec::new();
        let mut tokens = Vec::new();
        for _ in 0..24 {
            let mut context = f.context().await;
            let token = CancellationToken::new();
            context.cancel = Some(token.clone());
            effects.push(
                f.service
                    .submit(f.call(), &context)
                    .await
                    .unwrap()
                    .effect_id,
            );
            tokens.push(token);
        }
        let blocked = f.db.begin().await.unwrap();
        blocked
            .execute_unprepared("LOCK TABLE execution_effects IN ACCESS EXCLUSIVE MODE")
            .await
            .unwrap();
        let mut context = f.context().await;
        context.cancel = Some(CancellationToken::new());
        let service = f.service.clone();
        let call = f.call();
        let started = tokio::time::Instant::now();
        let submit = tokio::spawn(async move { service.submit(call, &context).await });
        wait_for_table_waiter(&f, "execution_effects").await;
        let service = f.service.clone();
        let shutdown = tokio::spawn(async move { service.shutdown().await });
        let result = tokio::time::timeout(Duration::from_secs(6), submit)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Err(ExecutionError::Database(_))));
        let error = tokio::time::timeout(Duration::from_secs(6), shutdown)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(11));
        assert!(tokens.iter().all(CancellationToken::is_cancelled));
        let error = error.to_string();
        for id in &effects {
            assert!(
                error.contains(&id.to_string()),
                "missing unfinished effect: {error}"
            );
        }
        assert!(matches!(f.service.health(), ModuleHealth::Unhealthy(_)));
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
        // Only after both bounded failures do we unblock PostgreSQL. A later
        // shutdown retries all retained ownership, while keeping the fault visible.
        blocked.rollback().await.unwrap();
        assert!(f.service.shutdown().await.is_err());
        for id in effects {
            f.status(id, EffectStatus::Cancelled).await;
        }
    })
    .await;
}

#[tokio::test]
async fn locked_ledger_cannot_extend_the_tool_result_wait_or_fabricate_cancellation() {
    with_fixture(false, 30, 0, |f| async move {
        f.service.start().await.unwrap();
        let mut context = f.context().await;
        let cancellation = CancellationToken::new();
        context.cancel = Some(cancellation.clone());
        let service = f.service.clone();
        let call = f.call();
        let execute = tokio::spawn(async move { service.execute(call, &context).await });
        f.calls(1).await;
        let blocked = f.db.begin().await.unwrap();
        blocked
            .execute_unprepared("LOCK TABLE execution_effects IN ACCESS EXCLUSIVE MODE")
            .await
            .unwrap();
        cancellation.cancel();
        wait_for_table_waiter(&f, "execution_effects").await;
        // Keep the table locked past the result deadline. Only the wait ends;
        // the externally dispatched operation remains owned by its worker.
        let result = tokio::time::timeout(Duration::from_secs(11), execute)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!result.success);
        assert_ne!(result.output["execution_status"], "cancelled");
        assert!(result.output["receipt"].is_null());
        let id = Uuid::parse_str(result.output["effect_id"].as_str().unwrap()).unwrap();
        blocked.rollback().await.unwrap();
        f.probe.permits.add_permits(1);
        f.status(id, EffectStatus::Succeeded).await;
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
    })
    .await;
}

async fn wait_for_table_waiter(f: &Fixture, table: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let row = f.admin.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "SELECT COUNT(*)::bigint AS waiting FROM pg_locks WHERE relation=$1::regclass AND NOT granted",
                vec![format!("{}.{table}", f.schema).into()])).await.unwrap().unwrap();
            if row.try_get::<i64>("", "waiting").unwrap() > 0 { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
}

/// Drops precisely the approval COMMIT acknowledgement, after PostgreSQL has
/// committed it. Everything else uses the real database protocol and service.
struct ApprovalAckProxy {
    db: DatabaseConnection,
    committed: Arc<tokio::sync::Notify>,
    release: CancellationToken,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl ApprovalAckProxy {
    async fn new(f: &Fixture, hold_ack: bool) -> Self {
        use sea_orm::sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};

        let options: PgConnectOptions = std::env::var("ALIA_TEST_DATABASE_URL")
            .unwrap()
            .parse()
            .unwrap();
        let upstream = (options.get_host().to_string(), options.get_port());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = CancellationToken::new();
        let release = CancellationToken::new();
        let committed = Arc::new(tokio::sync::Notify::new());
        let stop_task = stop.clone();
        let release_task = release.clone();
        let notify = committed.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            let commits = Arc::new(AtomicUsize::new(0));
            loop {
                tokio::select! {
                    biased;
                    _ = stop_task.cancelled() => break,
                    Some(_) = connections.join_next(), if !connections.is_empty() => {},
                    accepted = listener.accept() => {
                        let (client, _) = accepted.unwrap();
                        let upstream = upstream.clone();
                        let commits = commits.clone();
                        let notify = notify.clone();
                        let release = release_task.clone();
                        connections.spawn(async move {
                            let server = TcpStream::connect(upstream).await.unwrap();
                            // The proxy observes COMMIT acknowledgements; it must
                            // not add Nagle/delayed-ACK stalls to every PG frame.
                            client.set_nodelay(true).unwrap();
                            server.set_nodelay(true).unwrap();
                            let (mut client_read, mut client_write) = client.into_split();
                            let (mut server_read, mut server_write) = server.into_split();
                            let upload = tokio::io::copy(&mut client_read, &mut server_write);
                            let download = async {
                                loop {
                                    let mut header = [0u8; 5];
                                    server_read.read_exact(&mut header).await?;
                                    let length = u32::from_be_bytes(header[1..5].try_into().unwrap()) as usize;
                                    if !(4..=1024 * 1024).contains(&length) {
                                        return Err(std::io::Error::other("invalid PostgreSQL frame"));
                                    }
                                    let mut body = vec![0; length - 4];
                                    server_read.read_exact(&mut body).await?;
                                    if header[0] == b'C' && body == b"COMMIT\0"
                                        && commits.fetch_add(1, Ordering::SeqCst) == 1
                                    {
                                        notify.notify_one();
                                        if hold_ack { release.cancelled().await; }
                                        return Ok::<_, std::io::Error>(());
                                    }
                                    let mut frame = Vec::with_capacity(header.len() + body.len());
                                    frame.extend_from_slice(&header);
                                    frame.extend_from_slice(&body);
                                    client_write.write_all(&frame).await?;
                                }
                            };
                            tokio::select! { _ = upload => {}, _ = download => {} }
                        });
                    }
                }
            }
            connections.shutdown().await;
        });
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(2))
            .connect_with(
                options
                    .host("127.0.0.1")
                    .port(port)
                    .ssl_mode(PgSslMode::Disable)
                    .options([("search_path", format!("{},public", f.schema))]),
            )
            .await
            .unwrap();
        Self {
            db: sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool),
            committed,
            release,
            stop,
            task: Some(task),
        }
    }
    async fn close(mut self) {
        self.release.cancel();
        self.db.clone().close().await.unwrap();
        self.stop.cancel();
        self.task.take().unwrap().await.unwrap();
    }
}

impl Drop for ApprovalAckProxy {
    fn drop(&mut self) {
        self.release.cancel();
        self.stop.cancel();
    }
}

async fn approval_ack_case(f: Fixture, hold_ack: bool, drop_waiter: bool) {
    let proxy = ApprovalAckProxy::new(&f, hold_ack).await;
    let service = ExecutionService::new(
        ExecutionRepository::new(proxy.db.clone()),
        f.raw.clone(),
        &ToolsSection::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let token = CancellationToken::new();
    let mut context = f.context().await;
    context.cancel = Some(token.clone());
    let call = f.call();
    let submitting = service.clone();
    let submit = tokio::spawn(async move { submitting.submit(call, &context).await });
    tokio::time::timeout(Duration::from_secs(3), proxy.committed.notified())
        .await
        .unwrap();
    // Observed through a separate direct database connection: the approval is
    // already durable although the submitting client cannot know that fact.
    let effects = f.service.list(f.user, None, 10, None).await.unwrap();
    assert_eq!(effects.len(), 1);
    let effect = &effects[0];
    assert_eq!(effect.status, EffectStatus::Approved);
    if drop_waiter {
        submit.abort();
        assert!(submit.await.unwrap_err().is_cancelled());
    } else {
        let error = tokio::time::timeout(Duration::from_secs(6), submit)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(matches!(error, ExecutionError::Database(_)));
    }
    assert!(token.is_cancelled());
    proxy.release.cancel();
    service.start().await.unwrap();
    f.status(effect.effect_id, EffectStatus::Cancelled).await;
    assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
    service.shutdown().await.unwrap();
    proxy.close().await;
}

#[tokio::test]
async fn lost_approval_commit_ack_keeps_session_cancellation_ownership() {
    with_fixture(false, 5, 1, |f| approval_ack_case(f, false, false)).await;
}

#[tokio::test]
async fn dropped_submit_after_approval_commit_keeps_session_cancellation_ownership() {
    with_fixture(false, 5, 1, |f| approval_ack_case(f, true, true)).await;
}

#[tokio::test]
async fn approval_commit_ack_timeout_keeps_session_cancellation_ownership() {
    with_fixture(false, 5, 1, |f| approval_ack_case(f, true, false)).await;
}

#[tokio::test]
async fn shutdown_deadline_reports_live_worker_and_retains_its_eventual_receipt() {
    with_fixture(false, 30, 0, |f| async move {
        f.service.start().await.unwrap();
        let effect = f
            .service
            .submit(f.call(), &f.context().await)
            .await
            .unwrap();
        f.calls(1).await;
        let error = tokio::time::timeout(Duration::from_secs(11), f.service.shutdown())
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("worker_pending=true"));
        assert_eq!(f.probe.active.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.service
                .get(f.user, effect.effect_id)
                .await
                .unwrap()
                .status,
            EffectStatus::Dispatched
        );
        f.probe.permits.add_permits(1);
        // The first incomplete report remains visible, even once a later join
        // succeeds. The external call has never been aborted or repeated.
        assert!(f.service.shutdown().await.is_err());
        f.status(effect.effect_id, EffectStatus::Succeeded).await;
        assert_eq!(f.probe.active.load(Ordering::SeqCst), 0);
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
    })
    .await;
}
