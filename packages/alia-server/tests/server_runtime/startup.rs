//! Startup cancellation keeps the original waiter and cleanup owner until joined.
use super::*;
use alia_server::bootstrap::{ServerRuntime, ShutdownHandle};
use futures::future::LocalBoxFuture;
use std::net::SocketAddr;
use std::task::{Context as TaskContext, Wake, Waker};

#[path = "../delivery_support/mod.rs"]
mod case_support;

#[derive(Default)]
struct StartupContext {
    parent: CancellationToken,
    init_gate: Option<Arc<Semaphore>>,
    native_gate: Option<Arc<NativeGate>>,
    shutdown: Option<ShutdownHandle>,
    waiter: Option<LocalBoxFuture<'static, anyhow::Result<ServerRuntime>>>,
    runtime: Option<ServerRuntime>,
}

impl StartupContext {
    async fn wait(&mut self) -> anyhow::Result<()> {
        let result = self
            .waiter
            .as_mut()
            .expect("startup waiter installed")
            .await;
        self.waiter.take();
        self.runtime = Some(result?);
        Ok(())
    }

    async fn close(&mut self) -> bool {
        self.parent.cancel();
        if let Some(gate) = &self.init_gate {
            gate.add_permits(1);
        }
        if let Some(gate) = &self.native_gate {
            gate.release();
        }
        // A timed body only borrows this future. Retain and await its original
        // JoinHandle even when the body stopped waiting before ready delivery.
        if self.waiter.is_some() {
            let _ = self.wait().await;
        }
        let runtime_stopped = if let Some(runtime) = self.runtime.take() {
            matches!(
                AssertUnwindSafe(runtime.shutdown()).catch_unwind().await,
                Ok(Ok(()))
            )
        } else {
            true
        };
        let cleanup_finished = if let Some(handle) = &self.shutdown {
            // Expected startup errors remain in the report. Completion, rather
            // than an error-free report, proves resources actually left.
            let _ = handle.wait().await;
            handle.is_finished() && handle.pending_modules().is_empty()
        } else {
            true
        };
        runtime_stopped && cleanup_finished
    }
}

async fn with_startup<F>(test: F)
where
    F: for<'a> FnOnce(Fixture, &'a mut StartupContext) -> LocalBoxFuture<'a, ()>,
{
    let _permit = DATABASE_CASES.acquire().await.unwrap();
    let fixture = Fixture::new().await;
    let body_fixture = fixture.clone();
    case_support::run_context(
        StartupContext::default(),
        move |owner| test(body_fixture, owner),
        |mut owner| async move {
            assert!(
                owner.close().await,
                "startup context cleanup did not finish"
            );
        },
        || fixture.cleanup(),
    )
    .await;
}

struct Resources {
    fixture: Fixture,
    state: Arc<ProbeState>,
    application: String,
    http: SocketAddr,
    grpc: Option<SocketAddr>,
}

async fn start_blocked(
    owner: &mut StartupContext,
    mut fixture: Fixture,
    mut probe: Probe,
) -> Resources {
    let http_reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = http_reservation.local_addr().unwrap();
    fixture.config.server.port = http.port();
    #[cfg(feature = "channel-embedded")]
    let grpc_reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    #[cfg(feature = "channel-embedded")]
    let grpc = Some(grpc_reservation.local_addr().unwrap());
    #[cfg(not(feature = "channel-embedded"))]
    let grpc: Option<SocketAddr> = None;
    fixture.config.server.grpc_port = grpc.map(|addr| addr.port());
    drop(http_reservation);
    #[cfg(feature = "channel-embedded")]
    drop(grpc_reservation);

    let application = format!("startup_{}", Uuid::new_v4().simple());
    fixture
        .config
        .database
        .url
        .push_str(&format!("&application_name={application}"));
    let init_gate = Arc::new(Semaphore::new(0));
    owner.init_gate = Some(init_gate.clone());
    Arc::get_mut(&mut probe.state).unwrap().init_gate = Some(init_gate);
    owner.native_gate = probe.state.native_stop.clone();
    let state = probe.state.clone();
    let startup =
        ServerBuilder::from_config(Arc::new(fixture.config.clone()), owner.parent.clone())
            .with_extension(probe)
            .start_build();
    owner.shutdown = Some(startup.shutdown_handle());
    owner.waiter = Some(Box::pin(startup.wait()));
    Resources {
        fixture,
        state,
        application,
        http,
        grpc,
    }
}

struct ReadyWake(Arc<tokio::sync::Notify>);

impl Wake for ReadyWake {
    fn wake(self: Arc<Self>) {
        self.0.notify_one();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.notify_one();
    }
}

struct Buffered {
    initially_pending: bool,
    notified: bool,
}

impl Buffered {
    fn assert_observed(&self) {
        assert!(
            self.initially_pending,
            "startup output was consumed on first poll"
        );
        assert!(
            self.notified,
            "startup completion did not wake the registered waiter"
        );
    }
}

async fn buffer_result(owner: &mut StartupContext, resources: &Resources) -> Buffered {
    // Admission and identity work precede this real stage. The context still
    // owns the original, unpolled ready waiter while initialization is blocked.
    resources
        .state
        .initialized
        .acquire()
        .await
        .unwrap()
        .forget();
    let notification = Arc::new(tokio::sync::Notify::new());
    let waker = Waker::from(Arc::new(ReadyWake(notification.clone())));
    let mut context = TaskContext::from_waker(&waker);
    // Only this manual poll is unconstrained: a cooperative-budget wake must
    // not impersonate JoinHandle completion. Owner scheduling is unchanged.
    let initially_pending = Box::pin(tokio::task::unconstrained(
        owner.waiter.as_mut().unwrap().as_mut(),
    ))
    .as_mut()
    .poll(&mut context)
    .is_pending();
    owner.init_gate.as_ref().unwrap().add_permits(1);
    let notified = tokio::time::timeout(Duration::from_secs(5), notification.notified())
        .await
        .is_ok();
    Buffered {
        initially_pending,
        notified,
    }
}

struct ResourceEvidence {
    owned_pool_closed: bool,
    shared_pool_usable: bool,
    http_released: bool,
    grpc_released: bool,
    workers_left: usize,
    stops: usize,
}

impl ResourceEvidence {
    fn assert_reclaimed(&self) {
        assert_eq!(self.workers_left, 0, "startup worker survived rollback");
        assert_eq!(self.stops, 1, "startup worker was not stopped exactly once");
        assert!(
            self.owned_pool_closed,
            "owned startup pool survived rollback"
        );
        assert!(
            self.shared_pool_usable,
            "startup closed the fixture's separate pool"
        );
        assert!(
            self.http_released,
            "startup HTTP listener survived rollback"
        );
        assert!(
            self.grpc_released,
            "startup gRPC listener survived rollback"
        );
    }
}

impl Resources {
    async fn pool_alive(&self) -> bool {
        owned_pool_connections(&self.fixture.admin, &self.application).await > 0
    }

    async fn reclaimed(&self) -> ResourceEvidence {
        let owned_pool_closed = tokio::time::timeout(Duration::from_secs(2), async {
            while self.pool_alive().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        let shared_pool_usable = self.fixture.db.execute_unprepared("SELECT 1").await.is_ok();
        let http_released = tokio::net::TcpListener::bind(self.http).await.is_ok();
        let grpc_released = match self.grpc {
            Some(addr) => tokio::net::TcpListener::bind(addr).await.is_ok(),
            None => true,
        };
        let workers_left = self.state.active.load(Ordering::SeqCst);
        let stops = self.state.stops.load(Ordering::SeqCst);
        eprintln!(
            "startup resources: owned_pool_closed={owned_pool_closed}, shared_pool_usable={shared_pool_usable}, http_released={http_released}, grpc_released={grpc_released}, workers_left={workers_left}, stops={stops}"
        );
        ResourceEvidence {
            owned_pool_closed,
            shared_pool_usable,
            http_released,
            grpc_released,
            workers_left,
            stops,
        }
    }
}

#[tokio::test]
async fn cancelled_buffered_startup_rejects_ready_and_rolls_back() {
    with_startup(|fixture, owner| Box::pin(async move {
        let resources = start_blocked(owner, fixture,
            Probe::new("buffered_startup", ProbeState::new(Arc::default()))).await;
        let buffered = buffer_result(owner, &resources).await;
        let worker_active_before = resources.state.active.load(Ordering::SeqCst);
        let owned_pool_before = resources.pool_alive().await;
        let http_bound_before = tokio::net::TcpStream::connect(resources.http).await.is_ok();
        let grpc_bound_before = match resources.grpc {
            Some(addr) => tokio::net::TcpStream::connect(addr).await.is_ok(),
            None => true,
        };
        owner.parent.cancel();
        let startup_result = owner.wait().await;
        let returned_ready = startup_result.is_ok();
        let startup_error = startup_result.err().map(|error| format!("{error:#}"));
        let returned_pool = owner.runtime.as_ref().map(|runtime| runtime.state().db.clone());
        // Preserve old successful delivery evidence before fallback shutdown.
        let cleanup_completed = owner.close().await;
        let handle = owner.shutdown.as_ref().unwrap();
        let cleanup_finished = handle.is_finished() && handle.pending_modules().is_empty();
        let failures = handle.failures();
        let returned_pool_closed = match returned_pool {
            Some(pool) => pool.execute_unprepared("SELECT 1").await.is_err(),
            None => true,
        };
        let evidence = resources.reclaimed().await;
        eprintln!("buffered startup cancellation: ready_notified={}, returned_ready={returned_ready}, cleanup_finished={cleanup_finished}", buffered.notified);
        assert!(cleanup_completed && cleanup_finished, "original startup cleanup did not complete");
        evidence.assert_reclaimed();
        assert!(returned_pool_closed, "returned startup pool remained open");
        buffered.assert_observed();
        assert_eq!(worker_active_before, 1);
        assert!(owned_pool_before && http_bound_before && grpc_bound_before, "startup resources were not present before cancellation");
        assert!(!returned_ready, "cancelled startup delivered a buffered ready runtime");
        assert!(startup_error.as_deref().is_some_and(|error| error.contains("startup cancelled")), "cancellation was not returned as a startup failure");
        assert!(failures.iter().any(|error| error.contains("startup cancelled")), "cancellation was not retained by the original cleanup handle");
    })).await;
}

#[tokio::test]
async fn buffered_startup_cancellation_retains_incomplete_native_rollback() {
    with_startup(|fixture, owner| Box::pin(async move {
        let grace = Duration::from_millis(fixture.config.server.shutdown_grace_ms);
        let native_gate = Arc::new(NativeGate::default());
        let mut state = ProbeState::new(Arc::default());
        Arc::get_mut(&mut state).unwrap().native_stop = Some(native_gate);
        let resources = start_blocked(owner, fixture, Probe::new("buffered_native", state)).await;
        let buffered = buffer_result(owner, &resources).await;
        owner.parent.cancel();
        let started = Instant::now();
        let result = owner.wait().await;
        let elapsed = started.elapsed();
        let incomplete = result.as_ref().err()
            .and_then(|error| error.downcast_ref::<ShutdownIncomplete>()).cloned();
        let original = owner.shutdown.as_ref().unwrap().clone();
        let pending_module = original.pending_modules().iter().any(|name| name == "buffered_native");
        let unfinished = !original.is_finished();
        let pool_retained = resources.pool_alive().await;
        let worker_retained = resources.state.active.load(Ordering::SeqCst);
        let mut cancelled_waiter = Box::pin(original.wait());
        let wait_pending = futures::poll!(cancelled_waiter.as_mut()).is_pending();
        drop(cancelled_waiter);
        // close releases the same native gate before rejoining the same owner.
        let cleanup_completed = owner.close().await;
        let repeated_error = original.wait().await.is_err();
        let typed_handle_finished = if let Some(incomplete) = &incomplete {
            let result = incomplete.handle.wait().await;
            result.is_err() && incomplete.handle.is_finished()
        } else { false };
        let evidence = resources.reclaimed().await;
        eprintln!("buffered native rollback: incomplete={}, unfinished={unfinished}, pending_module={pending_module}, pool_retained={pool_retained}, wait_pending={wait_pending}, typed_handle_finished={typed_handle_finished}", incomplete.is_some());
        assert!(cleanup_completed, "native rollback cleanup did not finish");
        evidence.assert_reclaimed();
        buffered.assert_observed();
        assert!(incomplete.is_some(), "original grace must return typed ShutdownIncomplete");
        assert!(elapsed >= grace, "rollback did not cross its original grace");
        assert!(unfinished && pending_module && pool_retained && wait_pending);
        assert_eq!(worker_retained, 1);
        assert!(repeated_error && typed_handle_finished, "repeated original completion lost the startup failure");
        assert!(original.failures().iter().any(|error| error.contains("startup cancelled")));
    })).await;
}

#[tokio::test]
async fn dropping_unpolled_startup_wait_rolls_back_partial_initialization() {
    with_startup(|fixture, owner| Box::pin(async move {
        let resources = start_blocked(owner, fixture,
            Probe::new("unpolled_startup", ProbeState::new(Arc::default()))).await;
        resources.state.initialized.acquire().await.unwrap().forget();
        let worker_active_before = resources.state.active.load(Ordering::SeqCst);
        let pool_before = resources.pool_alive().await;
        let original = owner.shutdown.as_ref().unwrap().clone();
        // Deliberately cancel the public, never-polled wait future. Production
        // retains its independent rollback owner; its original handle stays here.
        drop(owner.waiter.take());
        let rollback_error = original.wait().await.err().map(|error| format!("{error:#}"));
        let parent_cancelled = owner.parent.is_cancelled();
        let cleanup_completed = owner.close().await;
        let evidence = resources.reclaimed().await;
        eprintln!("unpolled startup cancellation: rollback_finished={}, parent_cancelled={parent_cancelled}", original.is_finished());
        assert!(cleanup_completed && original.is_finished());
        evidence.assert_reclaimed();
        assert_eq!(worker_active_before, 1);
        assert!(pool_before, "partial startup never acquired its owned pool");
        assert!(!parent_cancelled, "dropping child startup cancelled its parent");
        assert!(rollback_error.as_deref().is_some_and(|error| error.contains("startup cancelled")));
    })).await;
}

#[tokio::test]
async fn buffered_startup_failures_survive_later_parent_cancellation() {
    with_startup(|fixture, owner| Box::pin(async move {
        let mut probe = Probe::new("buffered_failure", ProbeState::new(Arc::default()));
        probe.fail_init = true;
        probe.fail_stop = true;
        let resources = start_blocked(owner, fixture, probe).await;
        let buffered = buffer_result(owner, &resources).await;
        let original = owner.shutdown.as_ref().unwrap().clone();
        let finished_before_cancel = original.is_finished();
        let failures_before = original.failures();
        owner.parent.cancel();
        let result = owner.wait().await;
        let returned_ready = result.is_ok();
        let error = result.err().map(|error| format!("{error:#}"));
        let cleanup_completed = owner.close().await;
        let failures_after = original.failures();
        let repeated_error = original.wait().await.is_err();
        let evidence = resources.reclaimed().await;
        eprintln!("buffered startup failure: finished_before_cancel={finished_before_cancel}, returned_ready={returned_ready}, failure_report_unchanged={}", failures_before == failures_after);
        assert!(cleanup_completed && original.is_finished());
        evidence.assert_reclaimed();
        buffered.assert_observed();
        assert!(finished_before_cancel, "original failure was not buffered after rollback");
        assert!(!returned_ready, "failed initialization became ready");
        let error = error.expect("original startup failure missing");
        assert!(error.contains("injected init failure") && error.contains("injected stop failure"), "original initialization and stop errors were lost");
        assert!(!error.contains("startup cancelled"), "late cancellation replaced the original failure");
        assert_eq!(failures_before, failures_after);
        assert!(repeated_error && failures_after.iter().any(|error| error.contains("injected init failure")));
        assert!(failures_after.iter().any(|error| error.contains("injected stop failure")));
    })).await;
}
