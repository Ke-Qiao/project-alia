//! Real PostgreSQL and owned runtime resources. Missing infrastructure fails.
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use alia_server::bootstrap::ShutdownIncomplete;
use alia_server::config::ServerConfig;
use alia_server::event::EventBus;
use alia_server::modules::interaction::{cache::CacheStore, conversation::ConversationStore};
use alia_server::{bootstrap::ServerBuilder, modules::platform::lifecycle::ModuleLifecycle};
use async_trait::async_trait;
use futures::FutureExt;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

static DATABASE_CASES: Semaphore = Semaphore::const_new(2);

fn runtime_phase(phase: &str) {
    static START: std::sync::OnceLock<Option<Instant>> = std::sync::OnceLock::new();
    if let Some(start) =
        START.get_or_init(|| std::env::var_os("ALIA_TEST_RUNTIME_PHASES").map(|_| Instant::now()))
    {
        eprintln!(
            "[runtime-phase] pid={} unix_ms={} elapsed_ms={} phase={phase}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            start.elapsed().as_millis()
        );
    }
}

#[path = "server_runtime/websocket.rs"]
mod websocket;

#[path = "server_runtime/insights.rs"]
mod insights;

#[path = "server_runtime/source_epoch.rs"]
mod source_epoch;

#[path = "server_runtime/delivery.rs"]
mod delivery;

#[path = "server_runtime/expression.rs"]
mod expression;

#[path = "server_runtime/body.rs"]
mod body;

#[path = "server_runtime/context_policy.rs"]
mod context_policy;

#[path = "server_runtime/embedding_profile.rs"]
mod embedding_profile;

#[path = "server_runtime/relationship_api.rs"]
mod relationship_api;

#[path = "server_runtime/relationship_rebuild.rs"]
mod relationship_rebuild;

#[path = "server_runtime/companion.rs"]
mod companion;

#[path = "server_runtime/devices.rs"]
mod devices;

#[path = "server_runtime/skills.rs"]
mod skills;

#[cfg(feature = "llm-openai")]
#[path = "server_runtime/health.rs"]
mod health;

#[derive(Default)]
struct NativeGate {
    open: Mutex<bool>,
    changed: Condvar,
}
impl NativeGate {
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }
    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
    }
}
struct ReleaseOnDrop(Arc<NativeGate>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct ProbeState {
    log: Arc<Mutex<Vec<String>>>,
    initialized: Semaphore,
    stopped: Semaphore,
    active: AtomicUsize,
    stops: AtomicUsize,
    init_gate: Option<Arc<Semaphore>>,
    native_stop: Option<Arc<NativeGate>>,
    db: Mutex<Option<DatabaseConnection>>,
}
impl ProbeState {
    fn new(log: Arc<Mutex<Vec<String>>>) -> Arc<Self> {
        Arc::new(Self {
            log,
            initialized: Semaphore::new(0),
            stopped: Semaphore::new(0),
            active: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
            init_gate: None,
            native_stop: None,
            db: Mutex::new(None),
        })
    }
}
struct Probe {
    name: &'static str,
    state: Arc<ProbeState>,
    fail_init: bool,
    fail_start: bool,
    fail_stop: bool,
    cancel: CancellationToken,
    worker: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Probe {
    fn new(name: &'static str, state: Arc<ProbeState>) -> Self {
        Self {
            name,
            state,
            fail_init: false,
            fail_start: false,
            fail_stop: false,
            cancel: CancellationToken::new(),
            worker: tokio::sync::Mutex::new(None),
        }
    }
    fn with_database(self, db: DatabaseConnection) -> Self {
        *self.state.db.lock().unwrap() = Some(db);
        self
    }
}
#[async_trait]
impl ModuleLifecycle for Probe {
    fn name(&self) -> &'static str {
        self.name
    }
    async fn init(&mut self) -> anyhow::Result<()> {
        self.state
            .log
            .lock()
            .unwrap()
            .push(format!("init:{}", self.name));
        let state = self.state.clone();
        let cancel = self.cancel.clone();
        state.active.fetch_add(1, Ordering::SeqCst);
        *self.worker.lock().await = Some(tokio::spawn(async move {
            cancel.cancelled().await;
            state.active.fetch_sub(1, Ordering::SeqCst);
        }));
        self.state.initialized.add_permits(1);
        if let Some(gate) = &self.state.init_gate {
            gate.acquire().await.unwrap().forget();
        }
        anyhow::ensure!(!self.fail_init, "injected init failure");
        Ok(())
    }
    async fn start(&self) -> anyhow::Result<()> {
        self.state
            .log
            .lock()
            .unwrap()
            .push(format!("start:{}", self.name));
        anyhow::ensure!(!self.fail_start, "injected start failure");
        Ok(())
    }
    async fn stop(&self) -> anyhow::Result<()> {
        if let Some(gate) = &self.state.native_stop {
            gate.wait();
        }
        self.cancel.cancel();
        let mut worker = self.worker.lock().await;
        if let Some(task) = worker.as_mut() {
            task.await?;
            worker.take();
        }
        let db = self.state.db.lock().unwrap().clone();
        if let Some(db) = db {
            db.execute_unprepared("SELECT 1").await?;
        }
        self.state.stops.fetch_add(1, Ordering::SeqCst);
        self.state
            .log
            .lock()
            .unwrap()
            .push(format!("stop:{}", self.name));
        self.state.stopped.add_permits(1);
        anyhow::ensure!(!self.fail_stop, "injected stop failure after worker join");
        Ok(())
    }
}

#[cfg(not(feature = "insights"))]
#[async_trait]
impl alia_server::providers::adapter::TtsAdapter for Probe {
    async fn synthesize_request(
        &self,
        _: &alia_server::providers::adapter::TtsRequest,
        _: alia_server::modules::delivery::audio::ResponseAudioBudget,
    ) -> anyhow::Result<alia_server::modules::delivery::audio::EncodedAudio> {
        anyhow::bail!("runtime initialization probe must not synthesize audio")
    }

    async fn synthesize_stream_request(
        &self,
        _: &alia_server::providers::adapter::TtsRequest,
        _: alia_server::modules::delivery::audio::ResponseAudioBudget,
    ) -> anyhow::Result<alia_server::providers::adapter::TtsAudioStream> {
        anyhow::bail!("runtime initialization probe must not synthesize audio")
    }
}

#[derive(Clone)]
struct Fixture {
    admin: DatabaseConnection,
    db: DatabaseConnection,
    schema: String,
    dir: PathBuf,
    config: ServerConfig,
}
impl Fixture {
    async fn new() -> Self {
        runtime_phase("fixture.admin_connect.begin");
        let url = std::env::var("ALIA_TEST_DATABASE_URL")
            .expect("set ALIA_TEST_DATABASE_URL to disposable PostgreSQL");
        let admin = Database::connect(&url)
            .await
            .expect("test PostgreSQL is required");
        runtime_phase("fixture.admin_connect.done");
        let schema = format!("server_runtime_{}", Uuid::new_v4().simple());
        runtime_phase(&format!("fixture.create_schema.begin schema={schema}"));
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        runtime_phase("fixture.create_schema.done");
        let mut options = ConnectOptions::new(url.clone());
        options
            .max_connections(4)
            .set_schema_search_path(format!("{schema},public"))
            .sqlx_logging(false);
        runtime_phase("fixture.pool_connect.begin");
        let db = Database::connect(options).await.unwrap();
        runtime_phase("fixture.pool_connect.done");
        let dir = std::env::temp_dir().join(&schema);
        std::fs::create_dir(&dir).unwrap();
        let mut config: ServerConfig =
            toml::from_str(include_str!("../../../config/server.example.toml")).unwrap();
        config.config_dir = dir.clone();
        config.auth.server_keypair_path = dir.join("server.key");
        config.server.host = "127.0.0.1".into();
        config.server.port = 0;
        config.server.grpc_port = None;
        config.server.discovery_enabled = false;
        config.server.shutdown_grace_ms = 3_000;
        config.server.drain_timeout_ms = 20;
        config.database.url = format!(
            "{url}{}options=-csearch_path%3D{schema}%2Cpublic",
            if url.contains('?') { "&" } else { "?" }
        );
        config.database.max_connections = 4;
        config.database.min_connections = 0;
        config.channels.clear();
        config.modules.stt = "noop".into();
        config.modules.tts = "noop".into();
        config.llm.roles.minor = "noop".into();
        config.llm.roles.major = "noop".into();
        config.llm.roles.default = "noop".into();
        config.llm.roles.embedding = "noop".into();
        config.llm.roles.memory_capture = None;
        config.llm.roles.sentence_expression = None;
        config.policies.memory.enabled = false;
        config.policies.insights.enabled = false;
        config.plugins.home_assistant.enabled = false;
        config.providers.easy_turn.enabled = false;
        Self {
            admin,
            db,
            schema,
            dir,
            config,
        }
    }
    fn builder(&self) -> ServerBuilder {
        ServerBuilder::new(
            Arc::new(self.config.clone()),
            self.db.clone(),
            EventBus::new(64),
            CancellationToken::new(),
        )
    }
    async fn cleanup(self) {
        runtime_phase("fixture.close_pool.begin");
        self.db.close().await.unwrap();
        runtime_phase("fixture.close_pool.done");
        runtime_phase("fixture.drop_schema.begin");
        self.admin
            .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .await
            .unwrap();
        runtime_phase("fixture.drop_schema.done");
        runtime_phase("fixture.close_admin.begin");
        self.admin.close().await.unwrap();
        runtime_phase("fixture.close_admin.done");
        std::fs::remove_dir_all(self.dir).unwrap();
        runtime_phase("fixture.cleanup.done");
    }
}
async fn with_fixture<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    runtime_phase("fixture.permit.begin");
    let _permit = DATABASE_CASES.acquire().await.unwrap();
    runtime_phase("fixture.permit.done");
    let fixture = Fixture::new().await;
    runtime_phase("fixture.ready");
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(45),
        test(fixture.clone()),
    ))
    .catch_unwind()
    .await;
    runtime_phase("fixture.body.returned");
    fixture.cleanup().await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("runtime test exceeded deadline"),
        Err(error) => std::panic::resume_unwind(error),
    }
}
async fn signalled(signal: &Semaphore) {
    tokio::time::timeout(Duration::from_secs(5), signal.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}

#[tokio::test]
async fn partial_init_and_start_failures_stop_workers_once_in_reverse_order() {
    with_fixture(|f| async move {
        for fail_init in [true, false] {
            let log = Arc::new(Mutex::new(Vec::new()));
            let first = ProbeState::new(log.clone());
            let second = ProbeState::new(log.clone());
            let mut failing = Probe::new("second", second.clone()).with_database(f.db.clone());
            failing.fail_init = fail_init;
            failing.fail_start = !fail_init;
            let result = f
                .builder()
                .with_extension(Probe::new("first", first.clone()).with_database(f.db.clone()))
                .with_extension(failing)
                .build()
                .await;
            let error = result
                .err()
                .expect("required module failure must fail build");
            assert!(format!("{error:#}").contains(if fail_init {
                "injected init failure"
            } else {
                "injected start failure"
            }));
            assert_eq!(first.active.load(Ordering::SeqCst), 0);
            assert_eq!(second.active.load(Ordering::SeqCst), 0);
            assert_eq!(first.stops.load(Ordering::SeqCst), 1);
            assert_eq!(second.stops.load(Ordering::SeqCst), 1);
            let stopped = log
                .lock()
                .unwrap()
                .iter()
                .filter(|entry| entry.starts_with("stop:"))
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(stopped, ["stop:second", "stop:first"]);
            f.db.execute_unprepared("SELECT 1")
                .await
                .expect("injected pool remains owned by host");
        }
    })
    .await;
}

#[tokio::test]
async fn required_tool_catalog_failure_prevents_ready_and_can_be_retried() {
    with_fixture(|f| async move {
        alia_server::modules::platform::migrations::run(&f.db)
            .await
            .unwrap();
        f.db.execute_unprepared(
            "CREATE FUNCTION reject_tool_catalog() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'injected tool catalog failure'; END $$;
             CREATE TRIGGER reject_tool_catalog BEFORE INSERT OR UPDATE ON tools
             FOR EACH ROW EXECUTE FUNCTION reject_tool_catalog();",
        )
        .await
        .unwrap();
        let error = f
            .builder()
            .build()
            .await
            .err()
            .expect("catalog is required");
        assert!(format!("{error:#}").contains("injected tool catalog failure"));
        f.db.execute_unprepared("DROP TRIGGER reject_tool_catalog ON tools")
            .await
            .unwrap();
        let runtime = f.builder().build().await.unwrap();
        runtime.shutdown().await.unwrap();
        f.db.execute_unprepared("SELECT 1")
            .await
            .expect("borrowed pool remains usable");
    })
    .await;
}

#[tokio::test]
async fn cancelling_or_dropping_build_future_rolls_back_partial_init() {
    with_fixture(|f| async move {
        // Measure cancellation after reaching the blocked module. The first
        // real build identity hashes this debug test executable and belongs to
        // fixture setup, not the five-second module admission assertion.
        let identity_owner = alia_server::modules::execution::ExecutionService::new(
            alia_server::modules::execution::repository::ExecutionRepository::new(f.db.clone()),
            Arc::new(alia_server::tools::BuiltinToolRegistry::new()),
            &f.config.policies.tools,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        drop(identity_owner);
        for drop_future in [true, false] {
            let mut state = ProbeState::new(Arc::default());
            Arc::get_mut(&mut state).unwrap().init_gate = Some(Arc::new(Semaphore::new(0)));
            let parent = CancellationToken::new();
            let build = ServerBuilder::new(
                Arc::new(f.config.clone()),
                f.db.clone(),
                EventBus::new(64),
                parent.clone(),
            )
            .with_extension(Probe::new("blocked_init", state.clone()).with_database(f.db.clone()));
            let startup = build.start_build();
            let handle = startup.shutdown_handle();
            let task = tokio::spawn(startup.wait());
            signalled(&state.initialized).await;
            if drop_future {
                task.abort();
                let _ = task.await;
            } else {
                parent.cancel();
                assert!(task.await.unwrap().is_err());
            }
            signalled(&state.stopped).await;
            let error = handle.wait().await.unwrap_err();
            assert!(format!("{error:#}").contains("startup cancelled"));
            assert!(handle.is_finished());
            assert!(handle.pending_modules().is_empty());
            assert_eq!(state.active.load(Ordering::SeqCst), 0);
            if drop_future {
                assert!(!parent.is_cancelled());
            }
        }
    })
    .await;
}

#[tokio::test]
async fn native_stop_deadline_keeps_owner_and_pool_until_actual_completion() {
    with_fixture(|mut f| async move {
        f.config.server.shutdown_grace_ms = 30;
        let gate = Arc::new(NativeGate::default());
        let _release = ReleaseOnDrop(gate.clone());
        let mut state = ProbeState::new(Arc::default());
        Arc::get_mut(&mut state).unwrap().native_stop = Some(gate.clone());
        let runtime = ServerBuilder::from_config(Arc::new(f.config), CancellationToken::new())
            .with_extension(Probe::new("native_stop", state.clone()))
            .build()
            .await
            .unwrap();
        let pool = runtime.state().db.clone();
        *state.db.lock().unwrap() = Some(pool.clone());
        let handle = runtime.shutdown_handle();
        let started = Instant::now();
        let error = runtime.shutdown().await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1));
        let incomplete = error
            .downcast_ref::<ShutdownIncomplete>()
            .expect("typed incomplete retains completion handle");
        assert!(
            incomplete
                .pending_modules
                .iter()
                .any(|name| name == "native_stop")
        );
        assert!(!handle.is_finished());
        assert_eq!(state.active.load(Ordering::SeqCst), 1);
        pool.execute_unprepared("SELECT 1")
            .await
            .expect("dependent pool stays usable while stop is pending");
        gate.release();
        handle.wait().await.unwrap();
        assert_eq!(state.active.load(Ordering::SeqCst), 0);
        assert_eq!(state.stops.load(Ordering::SeqCst), 1);
        assert!(
            pool.execute_unprepared("SELECT 1").await.is_err(),
            "owned pool closes after dependent stop finishes"
        );
    })
    .await;
}

async fn owned_pool_connections(admin: &DatabaseConnection, application: &str) -> i64 {
    use sea_orm::{DatabaseBackend, Statement};
    admin
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS count FROM pg_stat_activity WHERE application_name = $1",
            vec![application.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "count")
        .unwrap()
}

#[tokio::test]
async fn failed_startup_returns_pending_handle_until_native_rollback_finishes() {
    with_fixture(|mut f| async move {
        f.config.server.shutdown_grace_ms = 30;
        let application = format!("rollback_{}", Uuid::new_v4().simple());
        f.config
            .database
            .url
            .push_str(&format!("&application_name={application}"));
        let gate = Arc::new(NativeGate::default());
        let _release = ReleaseOnDrop(gate.clone());
        let mut state = ProbeState::new(Arc::default());
        Arc::get_mut(&mut state).unwrap().native_stop = Some(gate.clone());
        let mut probe = Probe::new("partial_native", state.clone());
        probe.fail_init = true;
        let startup = ServerBuilder::from_config(Arc::new(f.config), CancellationToken::new())
            .with_extension(probe)
            .start_build();
        let handle = startup.shutdown_handle();
        let error = startup
            .wait()
            .await
            .err()
            .expect("init failure cannot become ready");
        let incomplete = error
            .downcast_ref::<ShutdownIncomplete>()
            .expect("rollback remains owned");
        assert!(
            incomplete
                .pending_modules
                .iter()
                .any(|name| name == "partial_native")
        );
        assert!(
            handle
                .failures()
                .iter()
                .any(|error| error.contains("injected init failure"))
        );
        assert!(
            owned_pool_connections(&f.admin, &application).await > 0,
            "owned pool stays connected while rollback is pending"
        );
        gate.release();
        assert!(
            handle.wait().await.is_err(),
            "startup failure is retained in final report"
        );
        assert!(handle.is_finished());
        assert_eq!(state.active.load(Ordering::SeqCst), 0);
        assert_eq!(state.stops.load(Ordering::SeqCst), 1);
        tokio::time::timeout(Duration::from_secs(2), async {
            while owned_pool_connections(&f.admin, &application).await != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("owned pool closes after rollback joins");
    })
    .await;
}

#[tokio::test]
async fn joined_stop_error_does_not_skip_remaining_cleanup_or_become_success() {
    with_fixture(|f| async move {
        let log = Arc::new(Mutex::new(Vec::new()));
        let first = ProbeState::new(log.clone());
        let second = ProbeState::new(log.clone());
        let mut failing = Probe::new("second", second.clone());
        failing.fail_stop = true;
        let runtime = ServerBuilder::from_config(Arc::new(f.config), CancellationToken::new())
            .with_extension(Probe::new("first", first.clone()))
            .with_extension(failing)
            .build()
            .await
            .unwrap();
        let pool = runtime.state().db.clone();
        *first.db.lock().unwrap() = Some(pool.clone());
        *second.db.lock().unwrap() = Some(pool.clone());
        let handle = runtime.shutdown_handle();
        let error = runtime.shutdown().await.unwrap_err();
        assert!(format!("{error:#}").contains("injected stop failure"));
        assert!(handle.is_finished());
        assert!(handle.pending_modules().is_empty());
        assert!(
            handle.wait().await.is_err(),
            "subsequent wait retains failure"
        );
        assert_eq!(first.active.load(Ordering::SeqCst), 0);
        assert_eq!(second.active.load(Ordering::SeqCst), 0);
        assert_eq!(first.stops.load(Ordering::SeqCst), 1);
        assert_eq!(second.stops.load(Ordering::SeqCst), 1);
        let stopped = log
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.starts_with("stop:"))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(stopped, ["stop:second", "stop:first"]);
        assert!(pool.execute_unprepared("SELECT 1").await.is_err());
    })
    .await;
}

#[tokio::test]
async fn independent_instances_keep_other_listener_and_shared_pool_alive() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    with_fixture(|f| async move {
        let parent = CancellationToken::new();
        let first = ServerBuilder::new(
            Arc::new(f.config.clone()),
            f.db.clone(),
            EventBus::new(64),
            parent.clone(),
        )
        .build()
        .await
        .unwrap();
        let mut second_config = f.config.clone();
        second_config.config_dir = f.dir.join("second");
        std::fs::create_dir(&second_config.config_dir).unwrap();
        second_config.auth.server_keypair_path = second_config.config_dir.join("server.key");
        let second = ServerBuilder::new(
            Arc::new(second_config),
            f.db.clone(),
            EventBus::new(64),
            parent.clone(),
        )
        .build()
        .await
        .unwrap();
        assert_ne!(first.http_addr(), second.http_addr());
        assert_ne!(
            first.state().config.auth.server_keypair_path,
            second.state().config.auth.server_keypair_path
        );
        let first_cancel = first.state().cancel_token.clone();
        let second_cancel = second.state().cancel_token.clone();
        let first_handle = first.shutdown_handle();
        let second_handle = second.shutdown_handle();
        let second_addr = second.http_addr();
        let first_run = tokio::spawn(first.run());
        let second_run = tokio::spawn(second.run());
        first_cancel.cancel();
        first_handle.wait().await.unwrap();
        first_run.await.unwrap().unwrap();
        assert!(!parent.is_cancelled());
        assert!(!second_cancel.is_cancelled());
        let mut socket = tokio::net::TcpStream::connect(second_addr).await.unwrap();
        socket
            .write_all(
                b"GET /api/v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 "));
        f.db.execute_unprepared("SELECT 1").await.unwrap();
        second_cancel.cancel();
        second_handle.wait().await.unwrap();
        second_run.await.unwrap().unwrap();
        f.db.execute_unprepared("SELECT 1").await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn dropping_run_closes_listeners_and_drains_owned_tasks_without_cancelling_parent() {
    with_fixture(|f| async move {
        let parent = CancellationToken::new();
        let state = ProbeState::new(Arc::default());
        let runtime = ServerBuilder::new(
            Arc::new(f.config.clone()),
            f.db.clone(),
            EventBus::new(64),
            parent.clone(),
        )
        .with_extension(Probe::new("running", state.clone()).with_database(f.db.clone()))
        .build()
        .await
        .unwrap();
        let http = runtime.http_addr();
        let grpc = runtime.grpc_addr();
        assert_ne!(http.port(), 0);
        assert_ne!(grpc.map(|addr| addr.port()), Some(1));
        assert_eq!(runtime.state().config.server.port, http.port());
        assert_eq!(runtime.requested_config.server.port, 0);
        let handle = runtime.shutdown_handle();
        let task = tokio::spawn(runtime.run());
        let socket = tokio::net::TcpStream::connect(http).await.unwrap();
        task.abort();
        let _ = task.await;
        handle.wait().await.unwrap();
        assert!(!parent.is_cancelled());
        assert_eq!(state.active.load(Ordering::SeqCst), 0);
        drop(socket);
        let _http = tokio::net::TcpListener::bind(http)
            .await
            .expect("HTTP port is released");
        if let Some(addr) = grpc {
            let _grpc = tokio::net::TcpListener::bind(addr)
                .await
                .expect("gRPC port is released");
        }
        f.db.execute_unprepared("SELECT 1").await.unwrap();
    })
    .await;
}

#[cfg(not(feature = "insights"))]
#[tokio::test]
async fn unavailable_insights_fail_before_tts_initialization() {
    with_fixture(|mut f| async move {
        f.config.policies.insights.enabled = true;
        let state = ProbeState::new(Arc::default());
        let error = f
            .builder()
            .with_tts(Probe::new("uninitialized_tts", state.clone()))
            .build()
            .await
            .err()
            .expect("enabled insights must require the insights feature");
        assert!(
            format!("{error:#}").contains("enabled insights require feature 'insights'"),
            "{error:#}"
        );
        assert_eq!(state.initialized.available_permits(), 0);
        assert_eq!(state.active.load(Ordering::SeqCst), 0);
    })
    .await;
}

#[tokio::test]
async fn occupied_listener_and_invalid_factory_fail_before_module_initialization() {
    with_fixture(|mut f| async move {
        let state = ProbeState::new(Arc::default());
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        f.config.server.port = occupied.local_addr().unwrap().port();
        assert!(
            f.builder()
                .with_extension(Probe::new("unused", state.clone()).with_database(f.db.clone()))
                .build()
                .await
                .is_err()
        );
        assert_eq!(state.initialized.available_permits(), 0);
        assert!(!f.config.auth.server_keypair_path.exists());
        f.config.server.port = 0;
        f.config.modules.stt = "nonexistent-provider".into();
        assert!(
            f.builder()
                .with_extension(Probe::new("unused", state.clone()).with_database(f.db.clone()))
                .build()
                .await
                .is_err()
        );
        assert_eq!(state.initialized.available_permits(), 0);
    })
    .await;
}

#[async_trait]
impl alia_server::modules::delivery::transport::DeliveryTransport for Probe {
    fn channel_id(&self) -> &str {
        self.name
    }
}

#[async_trait::async_trait]
impl alia_server::channels::ChannelAdapter for Probe {
    fn channel_type(&self) -> &'static str {
        "runtime_probe"
    }
    fn capabilities(&self) -> alia_server::channels::adapter::ChannelCapabilities {
        Default::default()
    }
    async fn execute_action(
        &self,
        _: alia_server::channels::adapter::ChannelAction,
    ) -> anyhow::Result<serde_json::Value> {
        anyhow::ensure!(
            self.state.active.load(Ordering::SeqCst) == 1,
            "channel stopped before effect finished"
        );
        Ok(serde_json::json!({"alive": true}))
    }
}

struct EffectProbe {
    release: Arc<Semaphore>,
    entered: Arc<Semaphore>,
    channels: Arc<alia_server::channels::ChannelRegistry>,
    plugin: Arc<ProbeState>,
}
#[async_trait]
impl alia_server::providers::adapter::ToolExecutor for EffectProbe {
    fn definition(&self) -> alia_server::providers::adapter::ToolDefinition {
        alia_server::providers::adapter::ToolDefinition {
            name: "runtime_effect_probe".into(),
            description: "Runtime dependency test".into(),
            parameters: serde_json::json!({"type":"object"}),
            routing_hints: vec![],
            source: "builtin".into(),
            require_confirmation: false,
            required_role: None,
        }
    }
    fn implementation_revision(&self) -> Option<String> {
        Some("runtime-effect-probe-v1".into())
    }
    async fn execute(
        &self,
        _: serde_json::Value,
        _ctx: &alia_server::providers::adapter::ToolContext,
    ) -> anyhow::Result<alia_server::providers::adapter::ToolResult> {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        anyhow::ensure!(
            self.plugin.active.load(Ordering::SeqCst) == 1,
            "plugin stopped before effect finished"
        );
        let channel = self
            .channels
            .get("outbound")
            .await
            .expect("outbound remains registered");
        channel
            .execute_action(alia_server::channels::adapter::ChannelAction {
                name: "test_dependency_alive".into(),
                params: serde_json::json!({}),
            })
            .await?;
        Ok(alia_server::providers::adapter::ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({"dependency_check":"alive"}),
            display: None,
        })
    }
}

#[tokio::test]
async fn dispatched_execution_retains_outbound_plugin_and_database_until_receipt() {
    use alia_server::modules::execution::types::EffectStatus;
    use alia_server::providers::adapter::{ToolCall, ToolContext};
    use sea_orm::{DatabaseBackend, Statement};
    with_fixture(|mut f| async move {
        f.config.server.shutdown_grace_ms = 40;
        let plugin = ProbeState::new(Arc::default());
        let outbound = ProbeState::new(Arc::default());
        let runtime = ServerBuilder::from_config(Arc::new(f.config), CancellationToken::new())
            .with_extension(Probe::new("plugin", plugin.clone()))
            .with_channel(Probe::new("outbound", outbound.clone())).build().await.unwrap();
        let state = runtime.state().clone();
        *plugin.db.lock().unwrap() = Some(state.db.clone());
        *outbound.db.lock().unwrap() = Some(state.db.clone());
        let release = Arc::new(Semaphore::new(0)); let entered = Arc::new(Semaphore::new(0));
        state.modules.tools.register_tool_executor("builtin", Arc::new(EffectProbe { release: release.clone(), entered: entered.clone(), channels: state.channels.clone(), plugin: plugin.clone() })).await.unwrap();
        let user = Uuid::new_v4();
        state.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Runtime test','unused','user','active')",
            vec![user.into(),format!("{user}@test.invalid").into()])).await.unwrap();
        let effect = state.execution.submit(ToolCall { tool_name: "runtime_effect_probe".into(), arguments: serde_json::json!({}), call_id: "call".into() }, &ToolContext {
            context_id: None,
            source_privacy_epoch: None,
            user_id: user, request_id: Uuid::new_v4(), invocation_key: "0".into(), source: "manual".into(), source_received_at: chrono::Utc::now(), source_event_id: None,
            conversation_id: None, channel_id: Some("outbound".into()), cancel: None,
        }).await.unwrap();
        signalled(&entered).await;
        let handle = runtime.shutdown_handle();
        let error = runtime.shutdown().await.unwrap_err();
        assert!(error.is::<ShutdownIncomplete>());
        assert_eq!(plugin.stops.load(Ordering::SeqCst), 0);
        assert_eq!(outbound.stops.load(Ordering::SeqCst), 0);
        assert!(state.channels.get("outbound").await.is_some());
        state.db.execute_unprepared("SELECT 1").await.unwrap();
        // Cross Execution's own ten-second shutdown wait as well as the short
        // host deadline. Its incomplete wait must not authorize dependency stop.
        tokio::time::sleep(Duration::from_millis(10_300)).await;
        assert!(!handle.is_finished());
        assert!(handle.pending_modules().iter().any(|name| name == "execution"));
        assert_eq!(plugin.stops.load(Ordering::SeqCst), 0);
        assert_eq!(outbound.stops.load(Ordering::SeqCst), 0);
        state.db.execute_unprepared("SELECT 1").await.unwrap();
        release.add_permits(1);
        let prior_timeout = handle.wait().await.unwrap_err();
        assert!(format!("{prior_timeout:#}").contains("execution"));
        assert!(handle.is_finished());
        assert!(handle.pending_modules().is_empty());
        assert_eq!(plugin.stops.load(Ordering::SeqCst), 1);
        assert_eq!(outbound.stops.load(Ordering::SeqCst), 1);
        let repository = alia_server::modules::execution::repository::ExecutionRepository::new(f.db.clone());
        let completed = repository.get(user, effect.effect_id).await.unwrap().unwrap();
        assert_eq!(completed.status, EffectStatus::Succeeded);
        assert!(state.db.execute_unprepared("SELECT 1").await.is_err());
    }).await;
}
