//! Real PostgreSQL tests of both projection gates against canonical source closures.
use alia_server::modules::memory::{MemoryNode, MemoryPatch, MemoryWriteOrigin, RetrievalOpts};
use std::{
    collections::BTreeMap,
    future::Future,
    panic::AssertUnwindSafe,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use alia_server::{
    config::{MemorySection, ServerConfig},
    expression::ExpressionRuntime,
    modules::{
        memory::{MemoryService, context::*, source::capture_source_epochs},
        platform::{lifecycle::ModuleLifecycle, migrations},
    },
    providers::{adapter::*, embedding::NoopEmbedding},
};
use async_trait::async_trait;
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::Value;
use tokio::sync::Semaphore;
use uuid::Uuid;

static CASES: Semaphore = Semaphore::const_new(2);
const ROOT: u128 = 10;
const MIDDLE: u128 = 20;
const TARGET: u128 = 30;
const SENTINEL: u128 = 100;
const TARGET_TEXT: &str = "canonical derived tea preference";
const UPDATED_TEXT: &str = "revised canonical derived tea preference";

fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}
fn id(value: u128) -> String {
    Uuid::from_u128(value).to_string()
}
fn node(value: u128, content: &str) -> MemoryNode {
    let mut node = MemoryNode::new("core", "fact", content, 0.9)
        .with_runtime_fields("semantic", "user", 0.9, 0.9, "internal");
    node.id = Some(id(value));
    node
}

struct Probe {
    profile: EmbeddingProfile,
    calls: Mutex<Vec<String>>,
    responses: Mutex<Vec<String>>,
    hold: Mutex<Option<String>>,
    entered: Semaphore,
    release: Semaphore,
    active: AtomicUsize,
}
impl Probe {
    fn new(dimensions: usize) -> Self {
        Self {
            profile: EmbeddingProfile {
                provider: "memory-provenance-fixture".into(),
                endpoint: "http://memory-provenance.test/v1".into(),
                model: format!("canonical-source-{dimensions}"),
                revision: "request-config.v1".into(),
                dimensions,
                distance: "cosine".into(),
                vector_type: "vector".into(),
            },
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(Vec::new()),
            hold: Mutex::new(None),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            active: AtomicUsize::new(0),
        }
    }
    fn calls_for(&self, text: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|v| *v == text)
            .count()
    }
    fn responses_for(&self, text: &str) -> usize {
        self.responses
            .lock()
            .unwrap()
            .iter()
            .filter(|v| *v == text)
            .count()
    }
    async fn wait_for_held_request(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.acquire())
            .await
            .expect("the production projection never reached its provider")
            .unwrap()
            .forget();
        assert_eq!(self.active.load(Ordering::SeqCst), 1);
    }
}
struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait]
impl ModuleLifecycle for Probe {
    fn name(&self) -> &'static str {
        "memory_provenance_probe"
    }
}
#[async_trait]
impl EmbeddingAdapter for Probe {
    async fn embed(&self, _: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        anyhow::bail!("canonical projection requires the actual profiled request")
    }
    async fn preview_profile_settings(
        &self,
        _: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Option<EmbeddingProfile>> {
        Ok(Some(self.profile.clone()))
    }
    async fn embed_profiled(&self, texts: &[String]) -> anyhow::Result<Option<ProfiledEmbeddings>> {
        self.calls.lock().unwrap().extend_from_slice(texts);
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(&self.active);
        let held = self
            .hold
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|hold| texts.contains(hold));
        if held {
            self.entered.add_permits(1);
            self.release.acquire().await?.forget();
        }
        let vectors = texts
            .iter()
            .map(|text| EmbeddingVector {
                input_digest: embedding_text_digest(text),
                values: (1..=self.profile.dimensions).map(|n| n as f32).collect(),
            })
            .collect();
        self.responses.lock().unwrap().extend_from_slice(texts);
        Ok(Some(ProfiledEmbeddings {
            profile: self.profile.clone(),
            vectors,
        }))
    }
    fn dimension(&self) -> usize {
        7
    } // Never authority for the real 3/4-dimensional request.
}

#[derive(Clone)]
struct Fixture {
    db: DatabaseConnection,
    admin: DatabaseConnection,
    schema: String,
    owner: Uuid,
    session: Uuid,
    memory: Arc<MemoryService>,
    expression: Arc<ExpressionRuntime>,
    probe: Arc<Probe>,
}
impl Fixture {
    async fn new(dimensions: usize) -> Self {
        let config = ServerConfig::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/server.example.toml"),
        )
        .unwrap();
        let url = std::env::var("ALIA_TEST_DATABASE_URL")
            .expect("set the disposable real PostgreSQL ALIA_TEST_DATABASE_URL");
        let mut options = ConnectOptions::new(url.clone());
        options.max_connections(1).sqlx_logging(false);
        let admin = Database::connect(options).await.unwrap();
        let schema = format!("memory_embedding_provenance_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut options = ConnectOptions::new(url);
        options
            .max_connections(6)
            .sqlx_logging(false)
            .set_schema_search_path(format!("{schema},public"));
        let db = match Database::connect(options).await {
            Ok(db) => db,
            Err(error) => {
                admin
                    .execute_unprepared(&format!("DROP SCHEMA {schema} CASCADE"))
                    .await
                    .unwrap();
                admin.close().await.unwrap();
                panic!("provenance schema connection: {error}");
            }
        };
        let f = Self {
            db,
            admin,
            schema,
            owner: Uuid::new_v4(),
            session: Uuid::new_v4(),
            memory: Arc::new(MemoryService::new()),
            expression: Arc::new(ExpressionRuntime::from_config(&config)),
            probe: Arc::new(Probe::new(dimensions)),
        };
        let setup = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(45), async {
            migrations::run(&f.db).await.unwrap();
            f.db.execute(sql(
                "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Memory provenance test','not-a-login','user','active')",
                vec![f.owner.into(),format!("{}@memory-provenance.test",f.owner).into()],
            )).await.unwrap();
            f.db.execute(sql(
                "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'memory-provenance-device',$2)",
                vec![f.session.into(),f.owner.to_string().into()],
            )).await.unwrap();
            f.memory.attach_runtime_for_test(f.db.clone(),Some(f.probe.clone()),MemorySection::default()).await.unwrap();
            f.policy(|policy| {
                policy.active_embedding_profile = None;
                policy.cache_enabled = false;
                policy.cache_embedding_profile_id = None;
            }).await;
            f.expression.attach_db(f.db.clone()).await.unwrap();
        })).catch_unwind().await;
        match setup {
            Ok(Ok(())) => f,
            result => {
                f.cleanup().await;
                match result {
                    Err(error) => std::panic::resume_unwind(error),
                    _ => panic!("Memory provenance setup exceeded 45 seconds"),
                }
            }
        }
    }
    fn service(&self) -> ContextService {
        ContextService::new(self.db.clone(), Arc::new(NoopEmbedding))
    }
    async fn policy(&self, edit: impl FnOnce(&mut ContextPolicy)) {
        let store = ContextPolicyStore::new(self.db.clone());
        let current = store.current().await.unwrap();
        let mut next = current.policy;
        edit(&mut next);
        store.replace(current.version, next).await.unwrap();
    }
    async fn enable_projection(&self) {
        let profile = self.probe.profile.id().unwrap();
        if ContextPolicyStore::new(self.db.clone())
            .current()
            .await
            .unwrap()
            .policy
            .active_embedding_profile
            .as_deref()
            == Some(profile.as_str())
        {
            return;
        }
        self.policy(|policy| policy.active_embedding_profile = Some(profile))
            .await;
    }
    async fn store(&self, value: u128, text: &str, messages: &[Uuid], parents: &[(String, i64)]) {
        let mut write = self.memory.write_snapshot(self.owner).await.unwrap();
        write.source_event_ids = messages.to_vec();
        write.source_memories = parents.to_vec();
        assert_eq!(
            self.memory
                .store_guarded(&write, node(value, text))
                .await
                .unwrap(),
            id(value)
        );
    }
    async fn parent_chain(&self) {
        self.store(ROOT, "canonical tea source", &[], &[]).await;
        self.store(MIDDLE, "intermediate tea fact", &[], &[(id(ROOT), 1)])
            .await;
        self.store(TARGET, TARGET_TEXT, &[], &[(id(MIDDLE), 1)])
            .await;
    }
    async fn source(&self, text: &str) -> ContextSource {
        let stamps = capture_source_epochs(&self.db, self.owner).await.unwrap();
        let source = Uuid::new_v4();
        let row = self.db.query_one(sql(
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,created_at,source_received_at,source_privacy_epoch,source_relationship_epoch) VALUES($1,$2,$3,'user',$4,clock_timestamp(),clock_timestamp(),$5,$6) RETURNING source_received_at",
            vec![source.into(),self.session.into(),self.owner.to_string().into(),text.into(),stamps.privacy_epoch.into(),stamps.relationship_epoch.into()],
        )).await.unwrap().unwrap();
        ContextSource {
            owner_id: self.owner,
            request_id: Uuid::new_v4(),
            channel_id: "memory-provenance-device".into(),
            conversation_id: Some(self.session),
            source_event_id: Some(source),
            source_received_at: row.try_get("", "source_received_at").unwrap(),
            source_privacy_epoch: stamps.privacy_epoch,
        }
    }
    async fn raw_message_chain(&self) -> Uuid {
        let source = self.source("I prefer tea").await.source_event_id.unwrap();
        self.store(
            ROOT,
            "canonical tea fact from the original message",
            &[source],
            &[],
        )
        .await;
        self.store(TARGET, TARGET_TEXT, &[], &[(id(ROOT), 1)]).await;
        source
    }
    async fn edit_source(&self, source: Uuid) {
        assert_eq!(self.db.execute(sql(
            "UPDATE chat_messages SET content='the original statement has been corrected' WHERE id=$1 AND user_id=$2",
            vec![source.into(),self.owner.to_string().into()],
        )).await.unwrap().rows_affected(),1);
    }
    async fn relationship_chain(&self) {
        // Keep the original Appraisal policy current throughout the race.
        // Canonical writes can commit with the adapter absent; restoring the
        // same descriptor must not advance policy or re-sign their sources.
        self.enable_projection().await;
        let version = ContextPolicyStore::new(self.db.clone())
            .current()
            .await
            .unwrap()
            .version;
        self.memory
            .attach_runtime_for_test(self.db.clone(), None, MemorySection::default())
            .await
            .unwrap();
        let source = self.source("thank you for helping me with tea").await;
        let generation = self
            .service()
            .materialize(source, "thank you for helping me with tea")
            .await
            .unwrap();
        let receipt = self
            .expression
            .observe_context_user(&self.service(), &generation)
            .await
            .unwrap();
        assert!(receipt.event_count > 0);
        let projected = self.service().extend_affect(&generation).await.unwrap();
        let write = self
            .memory
            .bind_context(
                self.memory.write_snapshot(self.owner).await.unwrap(),
                projected.lease().id(),
                MemoryWriteOrigin::Automatic,
            )
            .await
            .unwrap();
        self.memory
            .store_guarded(
                &write,
                node(ROOT, "tea fact influenced by the relationship"),
            )
            .await
            .unwrap();
        self.store(TARGET, TARGET_TEXT, &[], &[(id(ROOT), 1)]).await;
        self.memory
            .attach_runtime_for_test(
                self.db.clone(),
                Some(self.probe.clone()),
                MemorySection::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            ContextPolicyStore::new(self.db.clone())
                .current()
                .await
                .unwrap()
                .version,
            version
        );
        assert!(self.probe.calls.lock().unwrap().is_empty());
    }
    async fn clear_relationship(&self) -> RelationshipClearReceipt {
        self.service()
            .clear_relationship(self.owner, Uuid::new_v4(), 0)
            .await
            .unwrap()
    }
    async fn vector_count(&self, value: u128) -> i64 {
        self.db
            .query_one(sql(
            "SELECT count(*) AS n FROM memory_embeddings e JOIN memory_items m ON m.id=e.memory_id WHERE e.memory_id=$1 AND m.user_id=$2",
                vec![Uuid::from_u128(value).into(), self.owner.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap()
    }
    async fn assert_vector(&self, value: u128, revision: i64, text: &str) {
        let row = self.db.query_one(sql(
            "SELECT e.profile_id,e.dimensions,e.source_revision,e.input_digest,vector_dims(e.embedding) AS actual_dimensions FROM memory_embeddings e JOIN memory_items m ON m.id=e.memory_id WHERE e.memory_id=$1 AND m.user_id=$2",
            vec![Uuid::from_u128(value).into(),self.owner.into()],
        )).await.unwrap().expect("expected a real committed dynamic vector");
        assert_eq!(
            row.try_get::<String>("", "profile_id").unwrap(),
            self.probe.profile.id().unwrap()
        );
        assert_eq!(
            row.try_get::<i32>("", "dimensions").unwrap(),
            self.probe.profile.dimensions as i32
        );
        assert_eq!(
            row.try_get::<i32>("", "actual_dimensions").unwrap(),
            self.probe.profile.dimensions as i32
        );
        assert_eq!(row.try_get::<i64>("", "source_revision").unwrap(), revision);
        assert_eq!(
            row.try_get::<String>("", "input_digest").unwrap(),
            embedding_text_digest(text)
        );
    }
    async fn scan_past_target(&self) {
        // All inline writes have returned before start. The single rebuild
        // task awaits each ordered seed's execute; this final row proves the
        // target finished its turn without relying on shutdown cancellation.
        assert_eq!(self.vector_count(SENTINEL).await, 0);
        self.memory.start().await.unwrap();
        wait_until(|| async { self.vector_count(SENTINEL).await == 1 }).await;
        self.assert_vector(SENTINEL, 1, "independent scan sentinel")
            .await;
    }
    async fn sentinel(&self) {
        self.memory
            .attach_runtime_for_test(self.db.clone(), None, MemorySection::default())
            .await
            .unwrap();
        self.store(SENTINEL, "independent scan sentinel", &[], &[])
            .await;
        self.memory
            .attach_runtime_for_test(
                self.db.clone(),
                Some(self.probe.clone()),
                MemorySection::default(),
            )
            .await
            .unwrap();
        assert_eq!(self.vector_count(SENTINEL).await, 0);
    }
    async fn update_during<F, Fut>(&self, revoke: F)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ()>,
    {
        *self.probe.hold.lock().unwrap() = Some(UPDATED_TEXT.into());
        let target = id(TARGET);
        let (write, ()) = tokio::join!(
            self.memory.update(
                self.owner,
                &target,
                MemoryPatch {
                    expected_revision: Some(1),
                    content: Some(UPDATED_TEXT.into()),
                    ..Default::default()
                }
            ),
            async {
                self.probe.wait_for_held_request().await;
                assert_eq!(self.vector_count(TARGET).await, 0);
                revoke().await;
                assert_eq!(
                    self.probe.active.load(Ordering::SeqCst),
                    1,
                    "revocation must finish while the request is still held, not after its timeout"
                );
                self.probe.release.add_permits(1);
            }
        );
        write.unwrap(); // Canonical commit succeeds independently of projection.
        assert_eq!(self.probe.calls_for(UPDATED_TEXT), 1);
        assert_eq!(
            self.probe.responses_for(UPDATED_TEXT),
            1,
            "the actual provider response must reach the writeback gate"
        );
        assert_eq!(self.probe.active.load(Ordering::SeqCst), 0);
        assert_eq!(
            self.vector_count(TARGET).await,
            0,
            "completed projection owner must reject a revoked ancestor before SQL insert"
        );
    }
    async fn cleanup(self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            self.probe.release.close();
            let memory = self.memory.stop().await;
            let expression = self.expression.shutdown().await;
            let db = self.db.close().await;
            let schema = self
                .admin
                .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
                .await;
            let admin = self.admin.close().await;
            memory.unwrap();
            expression.unwrap();
            db.unwrap();
            schema.unwrap();
            admin.unwrap();
            assert_eq!(self.probe.active.load(Ordering::SeqCst), 0);
        })
        .await
        .expect("Memory provenance cleanup exceeded 10 seconds");
    }
}
async fn with_db<F, Fut>(dimensions: usize, test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    let _slot = CASES.acquire().await.unwrap();
    let f = Fixture::new(dimensions).await;
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(45),
        test(f.clone()),
    ))
    .catch_unwind()
    .await;
    f.cleanup().await;
    match result {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("Memory provenance test exceeded 45 seconds"),
        Err(error) => std::panic::resume_unwind(error),
    }
}
async fn wait_until<F, Fut>(mut ready: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if ready().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the independent worker sentinel was not committed");
}

#[derive(Clone, Copy, Debug)]
enum ParentChange {
    Revision,
    Private,
    Expired,
}
impl ParentChange {
    async fn apply(self, f: &Fixture) {
        if matches!(self, Self::Expired) {
            // Simulate an elapsed real database TTL without changing the
            // canonical revision or relying on the test host's wall clock.
            assert_eq!(f.db.execute(sql(
                "UPDATE memory_items SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1 AND user_id=$2 AND revision=1",
                vec![Uuid::from_u128(ROOT).into(),f.owner.into()],
            )).await.unwrap().rows_affected(),1);
        } else {
            let patch = match self {
                Self::Revision => MemoryPatch {
                    content: Some("corrected canonical coffee source".into()),
                    ..Default::default()
                },
                Self::Private => MemoryPatch {
                    visibility: Some("private_runtime".into()),
                    ..Default::default()
                },
                Self::Expired => unreachable!(),
            };
            f.memory
                .update(
                    f.owner,
                    &id(ROOT),
                    MemoryPatch {
                        expected_revision: Some(1),
                        ..patch
                    },
                )
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn invalid_parent_closure_is_not_exported_by_rebuild() {
    for change in [
        ParentChange::Revision,
        ParentChange::Private,
        ParentChange::Expired,
    ] {
        with_db(3, move |f| async move {
            f.parent_chain().await;
            f.sentinel().await;
            change.apply(&f).await;
            f.enable_projection().await;
            f.scan_past_target().await;
            assert_eq!(
                f.probe.calls_for(TARGET_TEXT),
                0,
                "before request: {change:?}"
            );
            assert_eq!(f.vector_count(TARGET).await, 0);
        })
        .await;
    }
}

#[tokio::test]
async fn parent_changed_during_actual_inference_rejects_writeback() {
    for change in [
        ParentChange::Revision,
        ParentChange::Private,
        ParentChange::Expired,
    ] {
        with_db(4, move |f| async move {
            f.parent_chain().await;
            f.enable_projection().await;
            f.update_during(|| change.apply(&f)).await;
        })
        .await;
    }
}

#[tokio::test]
async fn edited_raw_message_is_not_exported_by_rebuild() {
    with_db(4, |f| async move {
        let source = f.raw_message_chain().await;
        f.sentinel().await;
        f.edit_source(source).await;
        f.enable_projection().await;
        f.scan_past_target().await;
        assert_eq!(f.probe.calls_for(TARGET_TEXT), 0);
        assert_eq!(f.vector_count(TARGET).await, 0);
    })
    .await;
}

#[tokio::test]
async fn raw_message_edited_during_actual_inference_rejects_writeback() {
    with_db(3, |f| async move {
        let source = f.raw_message_chain().await;
        f.enable_projection().await;
        f.update_during(|| f.edit_source(source)).await;
    })
    .await;
}

#[tokio::test]
async fn relationship_clear_before_rebuild_skips_derived_memory() {
    with_db(3, |f| async move {
        f.relationship_chain().await;
        f.sentinel().await;
        assert_eq!(f.clear_relationship().await.removed_memories, 2);
        f.enable_projection().await;
        f.scan_past_target().await;
        assert_eq!(f.probe.calls_for(TARGET_TEXT), 0);
        assert_eq!(f.vector_count(TARGET).await, 0);
    })
    .await;
}

#[tokio::test]
async fn relationship_clear_during_actual_inference_rejects_writeback() {
    with_db(4, |f| async move {
        f.relationship_chain().await;
        f.enable_projection().await;
        f.update_during(|| async {
            assert_eq!(f.clear_relationship().await.removed_memories, 2);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn affect_disabled_and_unrelated_relationship_clear_preserve_ordinary_vectors() {
    with_db(4, |f| async move {
        let source = f
            .source("I prefer ordinary independent tea")
            .await
            .source_event_id
            .unwrap();
        f.store(ROOT, "ordinary source-backed tea fact", &[source], &[])
            .await;
        f.store(TARGET, TARGET_TEXT, &[], &[(id(ROOT), 1)]).await;
        f.sentinel().await;
        f.policy(|policy| policy.appraisal.affect.enabled = false)
            .await;
        assert_eq!(f.clear_relationship().await.removed_memories, 0);
        f.enable_projection().await;
        f.scan_past_target().await;
        f.assert_vector(ROOT, 1, "ordinary source-backed tea fact")
            .await;
        f.assert_vector(TARGET, 1, TARGET_TEXT).await;
        assert_eq!(f.probe.calls_for(TARGET_TEXT), 1);
        let rows = f
            .memory
            .retrieve(
                f.owner,
                "an unrelated semantic query",
                RetrievalOpts::default(),
            )
            .await
            .unwrap();
        assert!(
            rows.iter()
                .any(|row| row.id.as_deref() == Some(id(TARGET).as_str())
                    && row.metadata["retrieval_path"] == "profiled_vector")
        );
    })
    .await;
}
