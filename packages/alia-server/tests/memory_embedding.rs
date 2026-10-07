//! Canonical Memory -> actual request profile -> SQL projection, using real PG.
use alia_server::modules::memory::{MemoryNode, MemoryPatch, RetrievalOpts};
use std::{
    collections::BTreeMap,
    future::Future,
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use alia_server::{
    config::MemorySection,
    modules::{
        memory::{
            MemoryService,
            context::{ContextPolicy, ContextPolicyStore},
        },
        platform::{lifecycle::ModuleLifecycle, migrations},
    },
    providers::adapter::*,
};
use async_trait::async_trait;
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::Value;
use tokio::sync::{RwLock, Semaphore};
use uuid::Uuid;

static CASES: Semaphore = Semaphore::const_new(2);

fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}
fn profile(dimensions: usize, name: &str) -> EmbeddingProfile {
    EmbeddingProfile {
        provider: "memory-fixture".into(),
        endpoint: "http://memory.test/v1".into(),
        model: name.into(),
        revision: "test.v1".into(),
        dimensions,
        distance: "cosine".into(),
        vector_type: "vector".into(),
    }
}
fn node(content: &str) -> MemoryNode {
    MemoryNode::new("core", "fact", content, 0.9)
        .with_runtime_fields("semantic", "user", 0.9, 0.9, "internal")
}

#[derive(Clone)]
enum Mode {
    Normal,
    HoldFirst,
    HoldEvery,
    Delay(Duration),
    FailFirstText,
    WrongInput,
    WrongProfile,
    WrongCount,
    WrongDimensions,
    Zero,
    NonFinite,
}

struct Probe {
    profile: RwLock<Option<EmbeddingProfile>>,
    mode: RwLock<Mode>,
    calls: AtomicUsize,
    active: AtomicUsize,
    release: Semaphore,
}
impl Probe {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            profile: RwLock::new(Some(profile(1536, "initial"))),
            mode: RwLock::new(Mode::Normal),
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            release: Semaphore::new(0),
        })
    }
}
struct InFlight<'a>(&'a AtomicUsize);
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl ModuleLifecycle for Probe {
    fn name(&self) -> &'static str {
        "profiled_memory_probe"
    }
}
#[async_trait]
impl EmbeddingAdapter for Probe {
    async fn embed(&self, _: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        anyhow::bail!("Memory must not infer request identity from unprofiled embed")
    }
    async fn preview_profile_settings(
        &self,
        _: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Option<EmbeddingProfile>> {
        Ok(self.profile.read().await.clone())
    }
    async fn embed_profiled(&self, texts: &[String]) -> anyhow::Result<Option<ProfiledEmbeddings>> {
        let Some(mut profile) = self.profile.read().await.clone() else {
            return Ok(None);
        };
        let mode = self.mode.read().await.clone();
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = InFlight(&self.active);
        match mode {
            Mode::HoldFirst if call == 0 => self.release.acquire().await?.forget(),
            Mode::HoldEvery => self.release.acquire().await?.forget(),
            Mode::Delay(delay) => tokio::time::sleep(delay).await,
            Mode::FailFirstText if texts[0] == "fail-first" => {
                anyhow::bail!("deterministic bad first row")
            }
            _ => {}
        }
        let mut vectors = texts
            .iter()
            .map(|text| {
                let mut values = vec![0.0; profile.dimensions];
                values[0] = (call + 1) as f32;
                EmbeddingVector {
                    input_digest: embedding_text_digest(text),
                    values,
                }
            })
            .collect::<Vec<_>>();
        match mode {
            Mode::WrongInput => vectors[0].input_digest = embedding_text_digest("another input"),
            Mode::WrongProfile => profile.endpoint = "http://other.test".into(),
            Mode::WrongCount => vectors.clear(),
            Mode::WrongDimensions => {
                vectors[0].values.pop();
            }
            Mode::Zero => vectors[0].values.fill(0.0),
            Mode::NonFinite => vectors[0].values[0] = f32::NAN,
            _ => {}
        }
        Ok(Some(ProfiledEmbeddings { profile, vectors }))
    }
    fn dimension(&self) -> usize {
        7
    } // Deliberately wrong: this is not authority.
}

#[derive(Clone)]
struct Fixture {
    admin: DatabaseConnection,
    db: DatabaseConnection,
    schema: String,
    memory: Arc<MemoryService>,
    probe: Arc<Probe>,
    owner: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let url =
            std::env::var("ALIA_TEST_DATABASE_URL").expect("set disposable ALIA_TEST_DATABASE_URL");
        let admin = Database::connect(&url)
            .await
            .expect("PostgreSQL must be reachable");
        let schema = format!("memory_embedding_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut options = ConnectOptions::new(url);
        options
            .max_connections(5)
            .sqlx_logging(false)
            .set_schema_search_path(format!("{schema},public"));
        let db = Database::connect(options).await.unwrap();
        let f = Self {
            admin,
            db,
            schema,
            memory: Arc::new(MemoryService::new()),
            probe: Probe::new(),
            owner: Uuid::new_v4(),
        };
        if let Err(error) = migrations::run(&f.db).await {
            f.cleanup().await;
            panic!("Memory migrations: {error:#}");
        }
        f.add_owner(f.owner).await;
        f.memory
            .attach_runtime_for_test(
                f.db.clone(),
                Some(f.probe.clone()),
                MemorySection::default(),
            )
            .await
            .unwrap();
        f
    }
    async fn add_owner(&self, owner: Uuid) {
        self.db.execute(sql("INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Memory projection test','not-a-login','user','active')",
            vec![owner.into(),format!("{owner}@memory.test").into()])).await.unwrap();
    }
    async fn policy(&self, edit: impl FnOnce(&mut ContextPolicy)) -> i64 {
        let store = ContextPolicyStore::new(self.db.clone());
        let current = store.current().await.unwrap();
        let mut policy = current.policy;
        edit(&mut policy);
        store
            .replace(current.version, policy)
            .await
            .unwrap()
            .version
    }
    async fn set_profile(&self, next: Option<EmbeddingProfile>) -> i64 {
        *self.probe.profile.write().await = next.clone();
        // This installs only the immutable descriptor. Existing policy is not
        // overwritten by attaching/restarting a runtime.
        self.memory
            .attach_runtime_for_test(
                self.db.clone(),
                Some(self.probe.clone()),
                MemorySection::default(),
            )
            .await
            .unwrap();
        self.policy(|policy| {
            policy.active_embedding_profile = next.as_ref().map(|p| p.id().unwrap())
        })
        .await
    }
    async fn count(&self) -> i64 {
        self.db
            .query_one(sql("SELECT count(*) AS n FROM memory_embeddings", vec![]))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap()
    }
    async fn cleanup(self) {
        self.memory.stop().await.unwrap();
        self.db.close().await.unwrap();
        self.admin
            .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .await
            .unwrap();
        self.admin.close().await.unwrap();
    }
}
async fn with_db<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    let _slot = CASES.acquire().await.unwrap();
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(60),
        test(f.clone()),
    ))
    .catch_unwind()
    .await;
    f.cleanup().await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("Memory projection test exceeded deadline"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
async fn wait_until<F, Fut>(mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if check().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("condition was not reached");
}

#[tokio::test]
async fn actual_profiles_separate_dynamic_dimensions_and_cache_is_irrelevant() {
    with_db(|f| async move {
        f.policy(|p| { p.cache_enabled=false; p.cache_embedding_profile_id=None; }).await;
        for (index, dimensions) in [1024,1536,3072].into_iter().enumerate() {
            let next=profile(dimensions, &format!("model-{dimensions}"));
            f.set_profile(Some(next.clone())).await;
            let before=f.memory.retrieve(f.owner,"unrelated query",RetrievalOpts::default()).await.unwrap();
            assert!(before.iter().all(|row|row.metadata["retrieval_path"]!="profiled_vector"));
            let id=f.memory.store(f.owner,node(&format!("canonical source number {index}"))).await.unwrap();
            let rows=f.memory.retrieve(f.owner,"unrelated query",RetrievalOpts::default()).await.unwrap();
            assert_eq!(rows[0].id.as_deref(),Some(id.as_str()));
            assert_eq!(rows[0].metadata["retrieval_path"],"profiled_vector");
            let row=f.db.query_one(sql("SELECT dimensions,input_digest FROM memory_embeddings WHERE memory_id=$1::uuid AND profile_id=$2",
                vec![id.into(),next.id().unwrap().into()])).await.unwrap().unwrap();
            assert_eq!(row.try_get::<i32>("","dimensions").unwrap(),dimensions as i32);
            assert_eq!(row.try_get::<String>("","input_digest").unwrap(),embedding_text_digest(&format!("canonical source number {index}")));
        }
        assert_eq!(f.count().await,3);
        let other=Uuid::new_v4();f.add_owner(other).await;
        assert!(f.memory.retrieve(other,"unrelated query",RetrievalOpts::default()).await.unwrap().is_empty());
    }).await;
}

#[tokio::test]
async fn same_dimension_model_and_endpoint_changes_reject_late_old_projection() {
    with_db(|f| async move {
        *f.probe.mode.write().await=Mode::HoldFirst;
        let id=Uuid::new_v4();let mut input=node("first canonical version");input.id=Some(id.to_string());
        let memory=f.memory.clone();let owner=f.owner;
        let old=tokio::spawn(async move{memory.store(owner,input).await});
        wait_until(||async{f.probe.calls.load(Ordering::SeqCst)>=1}).await;
        assert!(f.memory.get(f.owner,&id.to_string()).await.unwrap().is_some());
        let mut next=profile(1536,"replacement-model");next.endpoint="http://replacement.test/v1".into();
        let version=f.set_profile(Some(next.clone())).await;
        f.memory.update(f.owner,&id.to_string(),MemoryPatch{content:Some("corrected canonical version".into()),..Default::default()}).await.unwrap();
        f.probe.release.add_permits(1);old.await.unwrap().unwrap();
        let row=f.db.query_one(sql("SELECT profile_id,source_revision,policy_version,input_digest FROM memory_embeddings",vec![])).await.unwrap().unwrap();
        assert_eq!(f.count().await,1);
        assert_eq!(row.try_get::<String>("","profile_id").unwrap(),next.id().unwrap());
        assert_eq!(row.try_get::<i64>("","source_revision").unwrap(),2);
        assert_eq!(row.try_get::<i64>("","policy_version").unwrap(),version);
        assert_eq!(row.try_get::<String>("","input_digest").unwrap(),embedding_text_digest("corrected canonical version"));
    }).await;
}

#[tokio::test]
async fn disable_then_reenable_does_not_refresh_old_work_but_rebuilds_current_memory() {
    with_db(|f| async move {
        *f.probe.mode.write().await = Mode::HoldFirst;
        let memory = f.memory.clone();
        let owner = f.owner;
        let old = tokio::spawn(async move {
            memory
                .store(owner, node("surviving canonical memory"))
                .await
        });
        wait_until(|| async { f.probe.calls.load(Ordering::SeqCst) == 1 }).await;
        f.policy(|p| p.memory_enabled = false).await;
        let version = f.policy(|p| p.memory_enabled = true).await;
        f.probe.release.add_permits(1);
        old.await.unwrap().unwrap();
        assert_eq!(f.count().await, 0);
        f.memory.start().await.unwrap();
        wait_until(|| async { f.count().await == 1 }).await;
        f.memory.stop().await.unwrap();
        let row =
            f.db.query_one(sql("SELECT policy_version FROM memory_embeddings", vec![]))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(row.try_get::<i64>("", "policy_version").unwrap(), version);
    })
    .await;
}

#[tokio::test]
async fn malformed_results_never_become_memory_vectors() {
    with_db(|f| async move {
        for (index, mode) in [
            Mode::WrongInput,
            Mode::WrongProfile,
            Mode::WrongCount,
            Mode::WrongDimensions,
            Mode::Zero,
            Mode::NonFinite,
        ]
        .into_iter()
        .enumerate()
        {
            *f.probe.mode.write().await = mode;
            let id = f
                .memory
                .store(
                    f.owner,
                    node(&format!("malformed projection source {index}")),
                )
                .await
                .unwrap();
            assert!(f.memory.get(f.owner, &id).await.unwrap().is_some());
            assert_eq!(f.count().await, 0);
        }
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 6);
    })
    .await;
}

#[tokio::test]
async fn private_memory_and_paused_projection_do_not_send_inference_requests() {
    with_db(|f| async move {
        let mut private = node("private canonical source");
        private.visibility = "private_runtime".into();
        f.memory.store(f.owner, private).await.unwrap();
        let mut sensitive = node("sensitive canonical source");
        sensitive.sensitive = true;
        f.memory.store(f.owner, sensitive).await.unwrap();
        f.policy(|p| p.paused_until = Some(chrono::Utc::now() + chrono::Duration::hours(1)))
            .await;
        f.memory
            .store(f.owner, node("manual memory while paused"))
            .await
            .unwrap();
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.count().await, 0);
    })
    .await;
}

#[tokio::test]
async fn slow_model_times_out_inline_then_finishes_owned_rebuild() {
    with_db(|f| async move {
        *f.probe.mode.write().await = Mode::Delay(Duration::from_millis(2200));
        let id = f
            .memory
            .store(f.owner, node("slow but legitimate model"))
            .await
            .unwrap();
        assert!(f.memory.get(f.owner, &id).await.unwrap().is_some());
        assert_eq!(f.count().await, 0);
        assert_eq!(f.probe.active.load(Ordering::SeqCst), 0);
        f.memory.start().await.unwrap();
        wait_until(|| async { f.count().await == 1 }).await;
        f.memory.stop().await.unwrap();
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 2);
        assert_eq!(f.probe.active.load(Ordering::SeqCst), 0);
    })
    .await;
}

#[tokio::test]
async fn failed_first_row_does_not_starve_later_rebuild_batches() {
    with_db(|f| async move {
        f.set_profile(None).await;
        for index in 1..=17u128 {
            let mut memory = node(&if index == 1 {
                "fail-first".into()
            } else {
                format!("fair rebuild source {index}")
            });
            memory.id = Some(Uuid::from_u128(index).to_string());
            f.memory.store(f.owner, memory).await.unwrap();
        }
        f.set_profile(Some(profile(1536, "rebuild"))).await;
        *f.probe.mode.write().await = Mode::FailFirstText;
        f.memory.start().await.unwrap();
        wait_until(|| async { f.count().await == 16 }).await;
        f.memory.stop().await.unwrap();
        assert!(
            f.db.query_one(sql(
                "SELECT memory_id FROM memory_embeddings WHERE memory_id=$1",
                vec![Uuid::from_u128(17).into()]
            ))
            .await
            .unwrap()
            .is_some()
        );
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 17);
    })
    .await;
}

#[tokio::test]
async fn duplicate_inline_and_rebuild_work_keeps_the_first_confirmed_vector() {
    with_db(|f| async move {
        *f.probe.mode.write().await = Mode::HoldFirst;
        let memory = f.memory.clone();
        let owner = f.owner;
        let store =
            tokio::spawn(async move { memory.store(owner, node("concurrent projection")).await });
        wait_until(|| async { f.probe.calls.load(Ordering::SeqCst) == 1 }).await;
        f.memory.start().await.unwrap();
        wait_until(|| async { f.count().await == 1 }).await;
        f.probe.release.add_permits(1);
        store.await.unwrap().unwrap();
        f.memory.stop().await.unwrap();
        let row =
            f.db.query_one(sql(
                "SELECT embedding::text AS value FROM memory_embeddings",
                vec![],
            ))
            .await
            .unwrap()
            .unwrap();
        assert!(
            row.try_get::<String>("", "value")
                .unwrap()
                .starts_with("[2,")
        );
        assert_eq!(f.count().await, 1);
    })
    .await;
}

#[tokio::test]
async fn stopping_rebuild_joins_inference_and_restart_repairs_missing_projection() {
    with_db(|f| async move {
        f.set_profile(None).await;
        f.memory
            .store(f.owner, node("restart projection"))
            .await
            .unwrap();
        f.set_profile(Some(profile(1536, "restart"))).await;
        *f.probe.mode.write().await = Mode::HoldEvery;
        f.memory.start().await.unwrap();
        wait_until(|| async { f.probe.active.load(Ordering::SeqCst) == 1 }).await;
        tokio::time::timeout(Duration::from_secs(1), f.memory.stop())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(f.probe.active.load(Ordering::SeqCst), 0);
        assert_eq!(f.count().await, 0);
        *f.probe.mode.write().await = Mode::Normal;
        let restarted = MemoryService::new();
        restarted
            .attach_runtime_for_test(
                f.db.clone(),
                Some(f.probe.clone()),
                MemorySection::default(),
            )
            .await
            .unwrap();
        restarted.start().await.unwrap();
        wait_until(|| async { f.count().await == 1 }).await;
        restarted.stop().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn standalone_query_cannot_refresh_epoch_after_clear() {
    with_db(|f| async move {
        f.memory
            .store(f.owner, node("original searchable memory"))
            .await
            .unwrap();
        let before = f.probe.calls.load(Ordering::SeqCst);
        *f.probe.mode.write().await = Mode::HoldEvery;
        let memory = f.memory.clone();
        let owner = f.owner;
        let query = tokio::spawn(async move {
            memory
                .retrieve(owner, "original", RetrievalOpts::default())
                .await
        });
        wait_until(|| async { f.probe.calls.load(Ordering::SeqCst) > before }).await;
        f.memory.clear(f.owner).await.unwrap();
        f.probe.release.add_permits(1);
        assert!(query.await.unwrap().is_err());
        assert_eq!(f.count().await, 0);
    })
    .await;
}

#[tokio::test]
async fn restart_with_uncommitted_profile_sends_no_embedding_requests() {
    with_db(|f| async move {
        f.memory
            .store(f.owner, node("old profile canonical source"))
            .await
            .unwrap();
        let calls = f.probe.calls.load(Ordering::SeqCst);
        *f.probe.profile.write().await = Some(profile(1536, "uncommitted-runtime-model"));
        let restarted = MemoryService::new();
        restarted
            .attach_runtime_for_test(
                f.db.clone(),
                Some(f.probe.clone()),
                MemorySection::default(),
            )
            .await
            .unwrap();
        restarted
            .store(f.owner, node("new source with unconfirmed runtime profile"))
            .await
            .unwrap();
        let rows = restarted
            .retrieve(f.owner, "canonical source", RetrievalOpts::default())
            .await
            .unwrap();
        assert!(
            rows.iter()
                .all(|row| row.metadata["retrieval_path"] != "profiled_vector")
        );
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), calls);
        assert_eq!(f.count().await, 1);
        restarted.stop().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn clear_while_rebuild_waits_rejects_its_original_epoch() {
    with_db(|f| async move {
        f.set_profile(None).await;
        f.memory
            .store(f.owner, node("clear this rebuilding canonical source"))
            .await
            .unwrap();
        f.set_profile(Some(profile(1536, "rebuild-before-clear")))
            .await;
        *f.probe.mode.write().await = Mode::HoldEvery;
        f.memory.start().await.unwrap();
        wait_until(|| async { f.probe.active.load(Ordering::SeqCst) == 1 }).await;
        tokio::time::timeout(Duration::from_secs(1), f.memory.clear(f.owner))
            .await
            .unwrap()
            .unwrap();
        f.probe.release.add_permits(1);
        wait_until(|| async { f.probe.active.load(Ordering::SeqCst) == 0 }).await;
        f.memory.stop().await.unwrap();
        assert_eq!(f.count().await, 0);
    })
    .await;
}

#[tokio::test]
async fn vector_sql_failure_keeps_canonical_write_and_surfaces_retrieval_failure() {
    with_db(|f|async move{
        f.db.execute_unprepared("CREATE FUNCTION fail_vector_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected vector failure'; END $$; CREATE TRIGGER fail_vector_insert BEFORE INSERT ON memory_embeddings FOR EACH ROW EXECUTE FUNCTION fail_vector_insert()").await.unwrap();
        let id=f.memory.store(f.owner,node("committed before vector failure")).await.unwrap();assert_eq!(f.count().await,0);
        assert!(f.memory.get(f.owner,&id).await.unwrap().is_some());
        f.db.execute_unprepared("DROP TRIGGER fail_vector_insert ON memory_embeddings").await.unwrap();
        f.memory.start().await.unwrap();wait_until(||async{f.count().await==1}).await;f.memory.stop().await.unwrap();
        f.db.execute(sql("UPDATE memory_embeddings SET input_digest=$1",vec![embedding_text_digest("wrong canonical body").into()])).await.unwrap();
        assert!(f.memory.retrieve(f.owner,"query",RetrievalOpts::default()).await.is_err());
    }).await;
}
