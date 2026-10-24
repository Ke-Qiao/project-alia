//! Real PostgreSQL tests; missing infrastructure is a failure, never a skip.
use std::{
    collections::VecDeque,
    future::Future,
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use alia_server::{
    modules::{
        embodiment::operations::*,
        execution::{
            repository::ExecutionRepository,
            types::{Approval, ApprovalKind, ClaimRequest, EffectKey, EffectRecord, NewEffect},
        },
        platform::migrations,
    },
    providers::adapter::ToolContext,
};
use async_trait::async_trait;
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use uuid::Uuid;

static CASES: Semaphore = Semaphore::const_new(2);

struct ReleaseOnDrop(Arc<Semaphore>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.add_permits(32);
    }
}

#[derive(Clone)]
struct Fixture {
    db: DatabaseConnection,
    admin: DatabaseConnection,
    schema: String,
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("ALIA_TEST_DATABASE_URL")
            .expect("set ALIA_TEST_DATABASE_URL to disposable PostgreSQL");
        let mut options = ConnectOptions::new(url.clone());
        options
            .max_connections(1)
            .connect_timeout(Duration::from_secs(5))
            .sqlx_logging(false);
        let admin = Database::connect(options)
            .await
            .expect("PostgreSQL test database reachable");
        let schema = format!("embodiment_test_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut options = ConnectOptions::new(url);
        options
            .max_connections(4)
            .connect_timeout(Duration::from_secs(5))
            .acquire_timeout(Duration::from_secs(5))
            .set_schema_search_path(format!("{schema},public"))
            .sqlx_logging(false);
        let db = Database::connect(options).await.unwrap();
        let fixture = Self { db, admin, schema };
        if let Err(error) = migrations::run(&fixture.db).await {
            fixture.cleanup().await;
            panic!("real migrations: {error:#}");
        }
        fixture
    }
    async fn cleanup(self) {
        self.db.close().await.unwrap();
        self.admin
            .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .await
            .unwrap();
        self.admin.close().await.unwrap();
    }
    async fn sql(&self, query: &str, values: Vec<sea_orm::Value>) {
        self.db
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                query,
                values,
            ))
            .await
            .unwrap();
    }
    async fn user(&self) -> Uuid {
        let id = Uuid::new_v4();
        self.sql("INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Physical fixture','not-a-login','user','active')",vec![id.into(),format!("{id}@physical.test").into()]).await;
        id
    }
    async fn count(&self, table: &str) -> i64 {
        assert!(
            table
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
        self.db
            .query_one(Statement::from_string(
                DatabaseBackend::Postgres,
                format!("SELECT COUNT(*)::bigint AS n FROM {table}"),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap()
    }
    async fn due(&self) {
        self.sql("UPDATE embodiment_operations SET next_observe_at=clock_timestamp()-interval '1 second'",vec![]).await;
    }
    async fn register(&self, service: &Arc<EmbodimentService>, adapter: Arc<FakeAdapter>) {
        service.register_adapter(adapter).unwrap();
        for entry in service.entries() {
            let d = entry.definition();
            self.sql("INSERT INTO tools(name,description,parameters,source,require_confirmation,enabled) VALUES($1,$2,$3,$4,$5,true) ON CONFLICT(name) DO NOTHING",vec![d.name.to_string().into(),d.description.clone().into(),d.parameters.clone().into(),d.source.into(),d.require_confirmation.into()]).await;
        }
    }
    async fn effect(&self, owner: Uuid, name: String, args: Value, dispatch: bool) -> EffectRecord {
        let repository = ExecutionRepository::new(self.db.clone());
        let effect = repository
            .propose(NewEffect {
                key: EffectKey {
                    user_id: owner,
                    request_id: Uuid::new_v4(),
                    invocation_key: "0".into(),
                },
                context_id: None,
                model_call_id: "physical-fixture".into(),
                source_received_at: chrono::Utc::now(),
                source_event_id: None,
                source_privacy_epoch: None,
                source: "integration_test".into(),
                conversation_id: None,
                channel_id: None,
                tool_name: name,
                arguments: args,
                tool_digest: "a".repeat(64),
                policy_digest: "b".repeat(64),
                requires_confirmation: true,
                required_role: None,
            })
            .await
            .unwrap();
        if dispatch {
            self.dispatch(&effect).await
        } else {
            effect
        }
    }
    async fn dispatch(&self, effect: &EffectRecord) -> EffectRecord {
        let repository = ExecutionRepository::new(self.db.clone());
        repository
            .approve(
                effect.key.user_id,
                effect.effect_id,
                Approval {
                    actor_id: effect.key.user_id,
                    kind: ApprovalKind::Explicit,
                    expected_intent_digest: effect.intent_digest.clone(),
                },
            )
            .await
            .unwrap();
        assert!(
            repository
                .claim(ClaimRequest {
                    effect_id: effect.effect_id,
                    worker_id: Uuid::new_v4(),
                    expected_tool_digest: effect.tool_digest.clone(),
                    expected_policy_digest: effect.policy_digest.clone(),
                    execution_timeout: Duration::from_secs(60)
                })
                .await
                .unwrap()
                .is_some()
        );
        repository
            .get(effect.key.user_id, effect.effect_id)
            .await
            .unwrap()
            .unwrap()
    }
}

fn context(effect: &EffectRecord) -> ToolContext {
    ToolContext {
        user_id: effect.key.user_id,
        context_id: effect.context_id,
        request_id: effect.key.request_id,
        source_received_at: effect.source_received_at,
        source_privacy_epoch: effect.source_privacy_epoch,
        source_event_id: effect.source_event_id,
        invocation_key: effect.key.invocation_key.clone(),
        conversation_id: effect.conversation_id,
        source: effect.source.clone(),
        channel_id: effect.channel_id.clone(),
        cancel: None,
    }
}

async fn with_database<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    let _permit = CASES.acquire().await.unwrap();
    let fixture = Fixture::new().await;
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(45),
        test(fixture.clone()),
    ))
    .catch_unwind()
    .await;
    fixture.cleanup().await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("physical operation test exceeded 45s"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[derive(Clone)]
enum Reply {
    State(OperationStatus, bool),
    Error,
    WrongBoot,
    Stale,
    Future,
    Replay,
    Foreign(OperationIdentity),
    Oversized,
}

struct FakeAdapter {
    descriptor: Mutex<AdapterDescriptor>,
    starts: AtomicUsize,
    observes: AtomicUsize,
    cancels: AtomicUsize,
    sequence: AtomicU64,
    replies: Mutex<VecDeque<Reply>>,
    start_gate: Mutex<Option<Arc<Semaphore>>>,
    cancel_gate: Mutex<Option<Arc<Semaphore>>>,
    observe_gate: Mutex<Option<Arc<Semaphore>>>,
    start_status: Mutex<OperationStatus>,
}
impl FakeAdapter {
    fn new(owner: Uuid, name: &str, device: Uuid, resource: &str) -> Arc<Self> {
        Arc::new(Self {
            descriptor: Mutex::new(AdapterDescriptor {
                owner_id: owner,
                device_id: device,
                boot_id: Uuid::new_v4(),
                adapter_id: name.into(),
                revision: "fixture-v1".into(),
                capability: "navigate".into(),
                capability_pin: "fixture-capability-v1".into(),
                goal_schema: json!({"type":"object","properties":{"destination":{"type":"string"}},"required":["destination"],"additionalProperties":false}),
                resources: vec![resource.into()],
            }),
            starts: AtomicUsize::new(0),
            observes: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
            sequence: AtomicU64::new(0),
            replies: Mutex::new(VecDeque::new()),
            start_gate: Mutex::new(None),
            cancel_gate: Mutex::new(None),
            observe_gate: Mutex::new(None),
            start_status: Mutex::new(OperationStatus::Accepted),
        })
    }
    fn reply(&self, reply: Reply) {
        self.replies.lock().unwrap().push_back(reply);
    }
    fn feedback(
        &self,
        identity: OperationIdentity,
        reply: Reply,
    ) -> anyhow::Result<OperationFeedback> {
        if matches!(reply, Reply::Error) {
            anyhow::bail!("fixture lost adapter acknowledgement");
        }
        let now = chrono::Utc::now().timestamp_millis();
        let mut feedback = OperationFeedback {
            identity,
            sequence: self.sequence.fetch_add(1, Ordering::SeqCst) + 1,
            observed_at_ms: now,
            status: OperationStatus::Running,
            payload: json!({"fixture":true}),
            stopped: None,
        };
        match reply {
            Reply::State(status, stopped) => {
                feedback.status = status;
                if stopped {
                    feedback.stopped = Some(StoppedEvidence {
                        observed_at_ms: now,
                        evidence: json!({"goal_fenced":true,"resource_control_released":true}),
                    });
                }
            }
            Reply::WrongBoot => feedback.identity.boot_id = Uuid::new_v4(),
            Reply::Stale => feedback.observed_at_ms -= 31_000,
            Reply::Future => feedback.observed_at_ms += 10_000,
            Reply::Replay => feedback.sequence = 1,
            Reply::Foreign(identity) => {
                feedback.identity = identity;
                feedback.status = OperationStatus::Succeeded;
                feedback.stopped = Some(StoppedEvidence {
                    observed_at_ms: now,
                    evidence: json!({"goal_fenced":true}),
                });
            }
            Reply::Oversized => feedback.payload = json!({"bytes":"x".repeat(20*1024)}),
            Reply::Error => unreachable!(),
        }
        Ok(feedback)
    }
}
#[async_trait]
impl PhysicalAdapter for FakeAdapter {
    fn descriptor(&self) -> AdapterDescriptor {
        self.descriptor.lock().unwrap().clone()
    }
    async fn start(&self, request: OperationRequest) -> anyhow::Result<OperationFeedback> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let gate = self.start_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.acquire().await?.forget();
        }
        self.feedback(
            request.identity,
            Reply::State(*self.start_status.lock().unwrap(), false),
        )
    }
    async fn observe(&self, identity: OperationIdentity) -> anyhow::Result<OperationFeedback> {
        self.observes.fetch_add(1, Ordering::SeqCst);
        let gate = self.observe_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.acquire().await?.forget();
        }
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Reply::State(OperationStatus::Running, false));
        self.feedback(identity, reply)
    }
    async fn cancel(&self, identity: OperationIdentity) -> anyhow::Result<OperationFeedback> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        let gate = self.cancel_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.acquire().await?.forget();
        }
        self.feedback(identity, Reply::State(OperationStatus::Cancelling, false))
    }
}

async fn wait_for(
    service: &EmbodimentService,
    owner: Uuid,
    id: Uuid,
    predicate: impl Fn(&OperationRecord) -> bool,
) -> OperationRecord {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let record = service.get(owner, id).await.unwrap().unwrap();
            if predicate(&record) {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected physical operation evidence")
}
async fn settle_counter(counter: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while counter.load(Ordering::SeqCst) < expected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
async fn begin(
    f: &Fixture,
    service: &Arc<EmbodimentService>,
    adapter: &FakeAdapter,
) -> (EffectRecord, OperationRecord) {
    let d = adapter.descriptor();
    let args = json!({"destination":"dock"});
    let effect = f
        .effect(d.owner_id, d.start_tool_name(), args.clone(), true)
        .await;
    let record = service
        .submit(&d.adapter_id, &context(&effect), args)
        .await
        .unwrap();
    (effect, record)
}

#[tokio::test]
async fn exact_dispatched_execution_and_principal_are_required_and_duplicate_is_one_operation() {
    with_database(|f| async move {
        let owner = f.user().await;
        let stranger = f.user().await;
        let service = EmbodimentService::new(f.db.clone());
        let adapter = FakeAdapter::new(owner, "auth", Uuid::new_v4(), "base");
        f.register(&service, adapter.clone()).await;
        assert!(
            service
                .entries()
                .iter()
                .filter(|e| e.principal().is_some())
                .all(|e| e.principal() == Some(owner) && e.definition().require_confirmation)
        );
        let args = json!({"destination":"dock"});
        let proposed = f
            .effect(
                owner,
                adapter.descriptor().start_tool_name(),
                args.clone(),
                false,
            )
            .await;
        assert!(
            service
                .submit("auth", &context(&proposed), args.clone())
                .await
                .is_err()
        );
        assert_eq!(f.count("embodiment_operations").await, 0);
        let dispatched = f.dispatch(&proposed).await;
        let caller = context(&dispatched);
        assert!(
            service
                .submit("auth", &caller, json!({"destination":"other"}))
                .await
                .is_err()
        );
        let mut forged = caller.clone();
        forged.request_id = Uuid::new_v4();
        assert!(service.submit("auth", &forged, args.clone()).await.is_err());
        forged = caller.clone();
        forged.user_id = stranger;
        assert!(service.submit("auth", &forged, args.clone()).await.is_err());
        let (a, b) = tokio::join!(
            service.submit("auth", &caller, args.clone()),
            service.submit("auth", &caller, args)
        );
        let a = a.unwrap();
        let b = b.unwrap();
        assert_eq!(a.identity, b.identity);
        assert_eq!(a.start_effect_id, dispatched.effect_id);
        let ready = wait_for(&service, owner, a.identity.operation_id, |r| {
            r.status == OperationStatus::Accepted
        })
        .await;
        assert!(ready.stopped.is_none());
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
        assert_eq!(f.count("embodiment_operations").await, 1);
        assert!(
            service
                .get(stranger, a.identity.operation_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(service.list(stranger, 100).await.unwrap().is_empty());
        f.sql(
            "UPDATE users SET status='disabled' WHERE id=$1",
            vec![owner.into()],
        )
        .await;
        assert!(service.get(owner, a.identity.operation_id).await.is_err());
        assert!(service.list(owner, 100).await.is_err());
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn accepted_and_cancelled_without_stop_keep_resources_and_new_cancel_can_stop_terminal_work()
{
    with_database(|f| async move {
        let owner = f.user().await;
        let adapter = FakeAdapter::new(owner, "resources", Uuid::new_v4(), "base");
        let first = EmbodimentService::new(f.db.clone());
        let second = EmbodimentService::new(f.db.clone());
        f.register(&first, adapter.clone()).await;
        f.register(&second, adapter.clone()).await;
        let (_, created) = begin(&f, &first, &adapter).await;
        let id = created.identity.operation_id;
        assert_eq!(created.status, OperationStatus::Submitting);
        wait_for(&first, owner, id, |r| r.status == OperationStatus::Accepted).await;
        let blocked = f
            .effect(
                owner,
                adapter.descriptor().start_tool_name(),
                json!({"destination":"dock"}),
                true,
            )
            .await;
        assert!(
            second
                .submit("resources", &context(&blocked), blocked.arguments.clone())
                .await
                .is_err()
        );
        let cancel = f
            .effect(
                owner,
                adapter.descriptor().cancel_tool_name(),
                json!({"operation_id":id}),
                true,
            )
            .await;
        let requested = second
            .cancel("resources", &context(&cancel), id)
            .await
            .unwrap();
        assert_eq!(requested.status, OperationStatus::Cancelling);
        second
            .cancel("resources", &context(&cancel), id)
            .await
            .unwrap();
        wait_for(&first, owner, id, |r| {
            r.last_confirmed_status == Some(OperationStatus::Cancelling)
        })
        .await;
        assert_eq!(adapter.cancels.load(Ordering::SeqCst), 1);
        assert_eq!(f.count("embodiment_operation_resources").await, 1);
        adapter.reply(Reply::State(OperationStatus::Cancelled, false));
        f.due().await;
        first.reconcile_once().await.unwrap();
        wait_for(&first, owner, id, |r| {
            r.status == OperationStatus::Cancelled
        })
        .await;
        assert_eq!(f.count("embodiment_operation_resources").await, 1);
        let stop = f
            .effect(
                owner,
                adapter.descriptor().cancel_tool_name(),
                json!({"operation_id":id}),
                true,
            )
            .await;
        assert_eq!(
            second
                .cancel("resources", &context(&stop), id)
                .await
                .unwrap()
                .status,
            OperationStatus::Cancelled
        );
        settle_counter(&adapter.cancels, 2).await;
        wait_for(&first, owner, id, |r| r.last_sequence >= 4).await;
        adapter.reply(Reply::State(OperationStatus::Cancelled, true));
        f.due().await;
        first.reconcile_once().await.unwrap();
        wait_for(&first, owner, id, |r| r.stopped.is_some()).await;
        assert_eq!(f.count("embodiment_operation_resources").await, 0);
        assert!(
            second
                .submit("resources", &context(&blocked), blocked.arguments.clone())
                .await
                .is_ok()
        );
        first.shutdown().await.unwrap();
        second.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn recovery_only_queries_and_unknown_or_new_boot_never_releases_resources() {
    with_database(|f| async move {
        let owner = f.user().await;
        let adapter = FakeAdapter::new(owner, "recovery", Uuid::new_v4(), "base");
        let first = EmbodimentService::new(f.db.clone());
        f.register(&first, adapter.clone()).await;
        let (_, record) = begin(&f, &first, &adapter).await;
        let id = record.identity.operation_id;
        wait_for(&first, owner, id, |r| r.status == OperationStatus::Accepted).await;
        adapter.reply(Reply::Error);
        f.due().await;
        first.reconcile_once().await.unwrap();
        wait_for(&first, owner, id, |r| r.status == OperationStatus::Unknown).await;
        first.shutdown().await.unwrap();
        let recovered = EmbodimentService::new(f.db.clone());
        f.register(&recovered, adapter.clone()).await;
        f.due().await;
        recovered.start().await.unwrap();
        wait_for(&recovered, owner, id, |r| {
            r.status == OperationStatus::Running
        })
        .await;
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
        assert!(recovered.register_adapter(adapter.clone()).is_err());
        recovered.shutdown().await.unwrap();
        adapter.descriptor.lock().unwrap().boot_id = Uuid::new_v4();
        let next_boot = EmbodimentService::new(f.db.clone());
        f.register(&next_boot, adapter.clone()).await;
        let observations = adapter.observes.load(Ordering::SeqCst);
        f.due().await;
        next_boot.reconcile_once().await.unwrap();
        let unknown = wait_for(&next_boot, owner, id, |r| {
            r.status == OperationStatus::Unknown
        })
        .await;
        assert_eq!(
            unknown.last_confirmed_status,
            Some(OperationStatus::Running)
        );
        assert_eq!(adapter.observes.load(Ordering::SeqCst), observations);
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
        assert_eq!(f.count("embodiment_operation_resources").await, 1);
        next_boot.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn feedback_rejects_stale_future_boot_regression_replay_and_oversized_payloads() {
    with_database(|f| async move {
        let owner = f.user().await;
        let adapter = FakeAdapter::new(owner, "feedback", Uuid::new_v4(), "base");
        let service = EmbodimentService::new(f.db.clone());
        f.register(&service, adapter.clone()).await;
        let (_, record) = begin(&f, &service, &adapter).await;
        let id = record.identity.operation_id;
        wait_for(&service, owner, id, |r| {
            r.status == OperationStatus::Accepted
        })
        .await;
        f.due().await;
        service.reconcile_once().await.unwrap();
        wait_for(&service, owner, id, |r| {
            r.status == OperationStatus::Running
        })
        .await;
        for reply in [
            Reply::Stale,
            Reply::Future,
            Reply::WrongBoot,
            Reply::State(OperationStatus::Accepted, false),
            Reply::Oversized,
        ] {
            let before = service.get(owner, id).await.unwrap().unwrap();
            adapter.reply(reply);
            f.due().await;
            service.reconcile_once().await.unwrap();
            let after = wait_for(&service, owner, id, |r| {
                r.updated_at_ms > before.updated_at_ms && r.status == OperationStatus::Unknown
            })
            .await;
            assert_eq!(after.last_sequence, 2);
            assert_eq!(after.last_confirmed_status, Some(OperationStatus::Running));
            assert!(after.stopped.is_none());
        }
        adapter.reply(Reply::Replay);
        f.due().await;
        service.reconcile_once().await.unwrap();
        settle_counter(&adapter.observes, 7).await;
        assert_eq!(f.count("embodiment_operation_resources").await, 1);
        adapter.reply(Reply::State(OperationStatus::Running, false));
        f.due().await;
        service.reconcile_once().await.unwrap();
        wait_for(&service, owner, id, |r| {
            r.status == OperationStatus::Running
        })
        .await;
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn feedback_for_a_different_existing_operation_cannot_release_its_resources() {
    with_database(|f| async move {
        let owner = f.user().await;
        let service = EmbodimentService::new(f.db.clone());
        let device = Uuid::new_v4();
        let source = FakeAdapter::new(owner, "source", device, "base");
        let target = FakeAdapter::new(owner, "target", device, "arm");
        f.register(&service, source.clone()).await;
        f.register(&service, target.clone()).await;
        let (_, a) = begin(&f, &service, &source).await;
        let (_, b) = begin(&f, &service, &target).await;
        wait_for(&service, owner, a.identity.operation_id, |r| {
            r.status == OperationStatus::Accepted
        })
        .await;
        wait_for(&service, owner, b.identity.operation_id, |r| {
            r.status == OperationStatus::Accepted
        })
        .await;
        source.reply(Reply::Foreign(b.identity.clone()));
        f.due().await;
        service.reconcile_once().await.unwrap();
        wait_for(&service, owner, a.identity.operation_id, |r| {
            r.status == OperationStatus::Unknown
        })
        .await;
        let target_record = service
            .get(owner, b.identity.operation_id)
            .await
            .unwrap()
            .unwrap();
        assert!(!target_record.status.is_terminal());
        assert!(target_record.stopped.is_none());
        assert_eq!(f.count("embodiment_operation_resources").await, 2);
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn cross_service_cancel_and_observe_wait_for_real_start_completion_after_timeout() {
    with_database(|f| async move {
        let owner = f.user().await;
        let adapter = FakeAdapter::new(owner, "ordering", Uuid::new_v4(), "base");
        let gate = Arc::new(Semaphore::new(0));
        let _release_on_failure = ReleaseOnDrop(gate.clone());
        *adapter.start_gate.lock().unwrap() = Some(gate.clone());
        let first = EmbodimentService::new(f.db.clone());
        let second = EmbodimentService::new(f.db.clone());
        f.register(&first, adapter.clone()).await;
        f.register(&second, adapter.clone()).await;
        let (_, record) = begin(&f, &first, &adapter).await;
        let id = record.identity.operation_id;
        settle_counter(&adapter.starts, 1).await;
        let cancel = f
            .effect(
                owner,
                adapter.descriptor().cancel_tool_name(),
                json!({"operation_id":id}),
                true,
            )
            .await;
        second
            .cancel("ordering", &context(&cancel), id)
            .await
            .unwrap();
        f.due().await;
        second.reconcile_once().await.unwrap();
        wait_for(&first, owner, id, |r| r.status == OperationStatus::Unknown).await;
        assert_eq!(adapter.cancels.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.observes.load(Ordering::SeqCst), 0);
        assert_eq!(f.count("embodiment_operation_resources").await, 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), first.shutdown())
                .await
                .is_err(),
            "shutdown must retain the pending start owner"
        );
        gate.add_permits(1);
        wait_for(&second, owner, id, |r| {
            r.last_confirmed_status == Some(OperationStatus::Cancelling)
        })
        .await;
        assert_eq!(adapter.cancels.load(Ordering::SeqCst), 1);
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
        first.shutdown().await.unwrap();
        second.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn full_normal_call_capacity_does_not_block_durable_cancel_admission() {
    with_database(|f| async move {
        let owner = f.user().await;
        let service = EmbodimentService::new(f.db.clone());
        let gate = Arc::new(Semaphore::new(0));
        let _release_on_failure = ReleaseOnDrop(gate.clone());
        let mut adapters = Vec::new();
        let mut records = Vec::new();
        for index in 0..8 {
            let adapter =
                FakeAdapter::new(owner, &format!("capacity_{index}"), Uuid::new_v4(), "base");
            *adapter.start_gate.lock().unwrap() = Some(gate.clone());
            f.register(&service, adapter.clone()).await;
            let (_, record) = begin(&f, &service, &adapter).await;
            adapters.push(adapter);
            records.push(record);
        }
        let descriptor = adapters[0].descriptor();
        let id = records[0].identity.operation_id;
        let cancel = f
            .effect(
                owner,
                descriptor.cancel_tool_name(),
                json!({"operation_id":id}),
                true,
            )
            .await;
        assert_eq!(
            service
                .cancel(&descriptor.adapter_id, &context(&cancel), id)
                .await
                .unwrap()
                .status,
            OperationStatus::Cancelling
        );
        assert_eq!(f.count("embodiment_operation_cancellations").await, 1);
        gate.add_permits(8);
        wait_for(&service, owner, id, |r| {
            r.last_confirmed_status == Some(OperationStatus::Cancelling)
        })
        .await;
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn cancel_timeout_retains_owner_then_accepts_late_cancelling_evidence() {
    with_database(|f| async move {
        let owner = f.user().await;
        let service = EmbodimentService::new(f.db.clone());
        let adapter = FakeAdapter::new(owner, "late_cancel", Uuid::new_v4(), "base");
        f.register(&service, adapter.clone()).await;
        let (_, record) = begin(&f, &service, &adapter).await;
        let id = record.identity.operation_id;
        wait_for(&service, owner, id, |r| {
            r.status == OperationStatus::Accepted
        })
        .await;
        let gate = Arc::new(Semaphore::new(0));
        let _release_on_failure = ReleaseOnDrop(gate.clone());
        *adapter.cancel_gate.lock().unwrap() = Some(gate.clone());
        let cancel = f
            .effect(
                owner,
                adapter.descriptor().cancel_tool_name(),
                json!({"operation_id":id}),
                true,
            )
            .await;
        service
            .cancel("late_cancel", &context(&cancel), id)
            .await
            .unwrap();
        wait_for(&service, owner, id, |r| {
            r.status == OperationStatus::Unknown
        })
        .await;
        assert_eq!(f.count("embodiment_operation_resources").await, 1);
        gate.add_permits(1);
        wait_for(&service, owner, id, |r| {
            r.status == OperationStatus::Cancelling
                && r.last_confirmed_status == Some(OperationStatus::Cancelling)
        })
        .await;
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn new_execution_cancel_after_restart_is_not_suppressed_by_persisted_cancelling() {
    with_database(|f| async move {
        let owner = f.user().await;
        let adapter = FakeAdapter::new(owner, "cancel_recovery", Uuid::new_v4(), "base");
        let first = EmbodimentService::new(f.db.clone());
        f.register(&first, adapter.clone()).await;
        let (_, record) = begin(&f, &first, &adapter).await;
        let id = record.identity.operation_id;
        wait_for(&first, owner, id, |r| r.status == OperationStatus::Accepted).await;
        let request = f
            .effect(
                owner,
                adapter.descriptor().cancel_tool_name(),
                json!({"operation_id":id}),
                true,
            )
            .await;
        first
            .cancel("cancel_recovery", &context(&request), id)
            .await
            .unwrap();
        wait_for(&first, owner, id, |r| {
            r.last_confirmed_status == Some(OperationStatus::Cancelling)
        })
        .await;
        first.shutdown().await.unwrap();
        let recovered = EmbodimentService::new(f.db.clone());
        f.register(&recovered, adapter.clone()).await;
        let retry = f
            .effect(
                owner,
                adapter.descriptor().cancel_tool_name(),
                json!({"operation_id":id}),
                true,
            )
            .await;
        recovered
            .cancel("cancel_recovery", &context(&retry), id)
            .await
            .unwrap();
        recovered
            .cancel("cancel_recovery", &context(&retry), id)
            .await
            .unwrap();
        settle_counter(&adapter.cancels, 2).await;
        assert_eq!(f.count("embodiment_operation_cancellations").await, 2);
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
        recovered.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn failed_operation_can_receive_authorized_stop_without_losing_its_failure() {
    with_database(|f| async move {
        let owner = f.user().await;
        let service = EmbodimentService::new(f.db.clone());
        let adapter = FakeAdapter::new(owner, "failed_stop", Uuid::new_v4(), "base");
        *adapter.start_status.lock().unwrap() = OperationStatus::Failed;
        f.register(&service, adapter.clone()).await;
        let (_, record) = begin(&f, &service, &adapter).await;
        let id = record.identity.operation_id;
        wait_for(&service, owner, id, |r| r.status == OperationStatus::Failed).await;
        assert_eq!(f.count("embodiment_operation_resources").await, 1);
        let cancel = f
            .effect(
                owner,
                adapter.descriptor().cancel_tool_name(),
                json!({"operation_id":id}),
                true,
            )
            .await;
        service
            .cancel("failed_stop", &context(&cancel), id)
            .await
            .unwrap();
        let acknowledged = wait_for(&service, owner, id, |r| r.last_sequence == 2).await;
        assert_eq!(acknowledged.status, OperationStatus::Failed);
        assert_eq!(
            acknowledged.last_confirmed_status,
            Some(OperationStatus::Failed)
        );
        assert!(acknowledged.stopped.is_none());
        adapter.reply(Reply::State(OperationStatus::Failed, true));
        f.due().await;
        service.reconcile_once().await.unwrap();
        wait_for(&service, owner, id, |r| r.stopped.is_some()).await;
        assert_eq!(f.count("embodiment_operation_resources").await, 0);
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn observation_timeout_keeps_original_owner_and_unknown_resource_reservation() {
    with_database(|f| async move {
        let owner = f.user().await;
        let service = EmbodimentService::new(f.db.clone());
        let adapter = FakeAdapter::new(owner, "observe_timeout", Uuid::new_v4(), "base");
        f.register(&service, adapter.clone()).await;
        let (_, record) = begin(&f, &service, &adapter).await;
        let id = record.identity.operation_id;
        wait_for(&service, owner, id, |r| {
            r.status == OperationStatus::Accepted
        })
        .await;
        let gate = Arc::new(Semaphore::new(0));
        let _release_on_failure = ReleaseOnDrop(gate.clone());
        *adapter.observe_gate.lock().unwrap() = Some(gate.clone());
        f.due().await;
        service.reconcile_once().await.unwrap();
        wait_for(&service, owner, id, |r| {
            r.status == OperationStatus::Unknown
        })
        .await;
        assert_eq!(f.count("embodiment_operation_resources").await, 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), service.shutdown())
                .await
                .is_err()
        );
        gate.add_permits(1);
        service.shutdown().await.unwrap();
        let record = service.get(owner, id).await.unwrap().unwrap();
        assert_eq!(record.status, OperationStatus::Running);
        assert!(record.stopped.is_none());
    })
    .await;
}
