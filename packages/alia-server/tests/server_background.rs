//! Lifecycle tests use explicit fake workers and no external device/database.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use alia_server::channels::{
    ChannelRegistry,
    adapter::{ChannelAdapter, ChannelCapabilities},
};
use alia_server::modules::platform::{background::OwnedBackground, lifecycle::ModuleLifecycle};
use async_trait::async_trait;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

const BOUND: Duration = Duration::from_secs(2);

struct OnDrop(Arc<AtomicUsize>);
impl Drop for OnDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn parent_cancellation_interrupts_endpoint_and_joins_worker() {
    let background = OwnedBackground::default();
    let parent = CancellationToken::new();
    let mut startup = background.begin_start(&parent).await.unwrap();
    let entered = Arc::new(Semaphore::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let (entered_job, finished_job) = (entered.clone(), finished.clone());
    startup.spawn_loop("pending endpoint", async move {
        let _finished = OnDrop(finished_job);
        entered_job.add_permits(1);
        std::future::pending::<()>().await;
    });
    startup.commit();
    entered.acquire().await.unwrap().forget();
    parent.cancel();
    tokio::time::timeout(BOUND, background.stop())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.load(Ordering::SeqCst), 1);
    assert!(
        background
            .begin_start(&CancellationToken::new())
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_stop_waiter_retains_native_cleanup_handle() {
    let background = OwnedBackground::default();
    let mut startup = background
        .begin_start(&CancellationToken::new())
        .await
        .unwrap();
    let cancel = startup.token();
    let release = Arc::new(Semaphore::new(0));
    let cleanup_started = Arc::new(Semaphore::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let (job_release, job_started, job_finished) =
        (release.clone(), cleanup_started.clone(), finished.clone());
    startup.spawn_owned("native cleanup", async move {
        cancel.cancelled().await;
        job_started.add_permits(1);
        job_release.acquire().await.unwrap().forget();
        job_finished.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    startup.commit();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), background.stop())
            .await
            .is_err()
    );
    cleanup_started.acquire().await.unwrap().forget();
    assert_eq!(finished.load(Ordering::SeqCst), 0);
    release.add_permits(1);
    tokio::time::timeout(BOUND, background.stop())
        .await
        .unwrap()
        .unwrap();
    background.stop().await.unwrap();
    assert_eq!(finished.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn dropped_startup_cancels_partial_workers_and_errors_survive_join() {
    let background = OwnedBackground::default();
    let mut startup = background
        .begin_start(&CancellationToken::new())
        .await
        .unwrap();
    let token = startup.token();
    let finished = Arc::new(AtomicUsize::new(0));
    let job_finished = finished.clone();
    startup.spawn_owned("partially started", async move {
        token.cancelled().await;
        job_finished.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("injected cleanup error")
    });
    drop(startup);
    assert!(
        background
            .stop()
            .await
            .unwrap_err()
            .to_string()
            .contains("injected cleanup error")
    );
    assert!(
        background
            .stop()
            .await
            .unwrap_err()
            .to_string()
            .contains("injected cleanup error")
    );
    assert_eq!(finished.load(Ordering::SeqCst), 1);
}

struct FakeChannel {
    id: String,
    start_entered: Semaphore,
    start_release: Semaphore,
    stop_entered: Semaphore,
    stop_release: Semaphore,
    starts: AtomicUsize,
    stops: AtomicUsize,
    stop_failure: bool,
}
impl FakeChannel {
    fn new(id: &str, pending_start: bool, pending_stop: bool, stop_failure: bool) -> Self {
        Self {
            id: id.into(),
            start_entered: Semaphore::new(0),
            start_release: Semaphore::new(usize::from(!pending_start)),
            stop_entered: Semaphore::new(0),
            stop_release: Semaphore::new(usize::from(!pending_stop)),
            starts: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
            stop_failure,
        }
    }
}
#[async_trait]
impl ModuleLifecycle for FakeChannel {
    fn name(&self) -> &'static str {
        "fake-channel"
    }
    async fn start(&self) -> anyhow::Result<()> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.start_entered.add_permits(1);
        self.start_release.acquire().await.unwrap().forget();
        Ok(())
    }
    async fn stop(&self) -> anyhow::Result<()> {
        self.stop_entered.add_permits(1);
        self.stop_release.acquire().await.unwrap().forget();
        self.stops.fetch_add(1, Ordering::SeqCst);
        anyhow::ensure!(!self.stop_failure, "injected stop failure");
        Ok(())
    }
}
#[async_trait]
impl ChannelAdapter for FakeChannel {
    fn channel_type(&self) -> &'static str {
        "fake"
    }
    fn channel_id(&self) -> &str {
        &self.id
    }
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities::default()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_start_waiter_cannot_detach_startup_or_restart_after_close() {
    let registry = Arc::new(ChannelRegistry::new());
    let channel = Arc::new(FakeChannel::new("pending", true, false, false));
    registry.register(channel.clone()).await.unwrap();
    let pending_registry = registry.clone();
    let waiter = tokio::spawn(async move { pending_registry.start("pending").await });
    channel.start_entered.acquire().await.unwrap().forget();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    tokio::time::timeout(BOUND, registry.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(channel.stops.load(Ordering::SeqCst), 1);
    assert!(registry.start("pending").await.is_err());
    assert!(
        registry
            .register(Arc::new(FakeChannel::new("new", false, false, false)))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn channel_stop_waiter_drop_retains_cleanup_and_requests_other_stops() {
    let registry = Arc::new(ChannelRegistry::new());
    let stalled = Arc::new(FakeChannel::new("stalled", false, true, false));
    let other = Arc::new(FakeChannel::new("other", false, false, false));
    registry.register(stalled.clone()).await.unwrap();
    registry.register(other.clone()).await.unwrap();
    // A disable already waiting for stop must not block requesting other stops.
    let disabling = registry.clone();
    let disable = tokio::spawn(async move { disabling.disable("stalled", "test").await });
    stalled.stop_entered.acquire().await.unwrap().forget();
    let shutting_down = registry.clone();
    let shutdown = tokio::spawn(async move { shutting_down.shutdown().await });
    tokio::time::timeout(BOUND, other.stop_entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    disable.abort();
    shutdown.abort();
    let _ = disable.await;
    let _ = shutdown.await;
    stalled.stop_release.add_permits(1);
    tokio::time::timeout(BOUND, registry.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stalled.stops.load(Ordering::SeqCst), 1);
    assert_eq!(other.stops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn same_id_replacement_requires_confirmed_successful_cleanup() {
    let registry = Arc::new(ChannelRegistry::new());
    let old = Arc::new(FakeChannel::new("device", false, true, false));
    registry.register(old.clone()).await.unwrap();
    let disabling = registry.clone();
    let disable = tokio::spawn(async move { disabling.disable("device", "restart").await });
    old.stop_entered.acquire().await.unwrap().forget();
    assert!(
        registry
            .register(Arc::new(FakeChannel::new("device", false, false, false)))
            .await
            .is_err()
    );
    old.stop_release.add_permits(1);
    disable.await.unwrap().unwrap();
    let replacement = Arc::new(FakeChannel::new("device", false, false, false));
    registry.register(replacement.clone()).await.unwrap();
    registry.start("device").await.unwrap();
    registry.shutdown().await.unwrap();
    assert_eq!(old.stops.load(Ordering::SeqCst), 1);
    assert_eq!(replacement.stops.load(Ordering::SeqCst), 1);

    let failed = ChannelRegistry::new();
    failed
        .register(Arc::new(FakeChannel::new("device", false, false, true)))
        .await
        .unwrap();
    assert!(failed.disable("device", "failed stop").await.is_err());
    assert!(
        failed
            .register(Arc::new(FakeChannel::new("device", false, false, false)))
            .await
            .is_err()
    );
    assert!(
        failed
            .shutdown()
            .await
            .unwrap_err()
            .to_string()
            .contains("injected stop failure")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_and_register_race_cannot_escape_shutdown_snapshot() {
    for _ in 0..32 {
        let registry = Arc::new(ChannelRegistry::new());
        let channel = Arc::new(FakeChannel::new("device", false, false, false));
        let gate = Arc::new(tokio::sync::Barrier::new(2));
        let (r, c, g) = (registry.clone(), channel.clone(), gate.clone());
        let register = tokio::spawn(async move {
            g.wait().await;
            r.register(c).await
        });
        gate.wait().await;
        registry.close_admission();
        let admitted = register.await.unwrap().is_ok();
        registry.shutdown().await.unwrap();
        assert_eq!(channel.stops.load(Ordering::SeqCst), usize::from(admitted));
        assert!(registry.start("device").await.is_err());
    }
}

#[tokio::test]
async fn sensor_cleanup_is_joined_and_cannot_restart() {
    use alia_server::modules::platform::sensors::{SensorRegistry, SensorRegistryConfig};
    let registry = SensorRegistry::new(SensorRegistryConfig::default());
    let parent = CancellationToken::new();
    registry.spawn_cleanup_loop(parent.clone()).await.unwrap();
    assert!(registry.spawn_cleanup_loop(parent).await.is_err());
    tokio::time::timeout(BOUND, registry.stop_cleanup_loop())
        .await
        .unwrap()
        .unwrap();
    assert!(
        registry
            .spawn_cleanup_loop(CancellationToken::new())
            .await
            .is_err()
    );
}

#[cfg(feature = "channel-embedded")]
#[tokio::test(flavor = "current_thread")]
async fn embedded_stop_interrupts_stalled_http2_connect_and_discovery() {
    use alia_server::channels::embedded::{EmbeddedChannel, EmbeddedChannelConfig};
    use alia_server::event::EventBus;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let channel = EmbeddedChannel::new(
        EmbeddedChannelConfig {
            id: "test-device".into(),
            owner_user_id: None,
            expected_device_id: None,
            expected_device_public_key: None,
            endpoint: format!("http://{}", listener.local_addr().unwrap()),
            discovery_service_name: format!("_alia-test-{}._tcp", uuid::Uuid::new_v4().simple()),
            discovery_timeout_sec: 60,
            reconnect_interval_sec: 1,
            reconnect_max_backoff_sec: 1,
            grpc_timeout_sec: 60,
        },
        EventBus::new(8),
    );
    channel.start().await.unwrap();
    // Accept TCP but never speak HTTP/2, so connect is still pending at stop.
    let (_socket, _) = tokio::time::timeout(BOUND, listener.accept())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(BOUND, channel.stop())
        .await
        .unwrap()
        .unwrap();
    assert!(channel.plugin_client().await.is_err());
    assert!(channel.start().await.is_err());
}

#[tokio::test]
async fn real_discovery_cancellation_waits_for_native_shutdown_ack() {
    let cancel = CancellationToken::new();
    let query_cancel = cancel.clone();
    let query = tokio::spawn(async move {
        alia_transport::discover_service_cancellable(
            "_alia-test-cancel._tcp",
            Duration::from_secs(60),
            &query_cancel,
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();
    // This function returns only after shutdown_discovery_daemon receives ACK.
    assert!(
        tokio::time::timeout(BOUND, query)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whole_restart_guard_prevents_id_operations_from_crossing_replacements() {
    let registry = Arc::new(ChannelRegistry::new());
    let first = Arc::new(FakeChannel::new("device", false, false, false));
    let second = Arc::new(FakeChannel::new("device", false, false, false));
    let first_guard = registry.restart_guard("device").await.unwrap();
    registry.register(first.clone()).await.unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let entered_second = Arc::new(Semaphore::new(0));
    let (r, c, b, entered) = (
        registry.clone(),
        second.clone(),
        barrier.clone(),
        entered_second.clone(),
    );
    let replacement = tokio::spawn(async move {
        b.wait().await;
        let _operation = r.restart_guard("device").await.unwrap();
        entered.add_permits(1);
        r.disable("device", "replacement").await.unwrap();
        r.register(c).await.unwrap();
        r.start("device").await.unwrap();
    });
    barrier.wait().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), entered_second.acquire())
            .await
            .is_err()
    );
    registry.start("device").await.unwrap();
    assert_eq!(first.starts.load(Ordering::SeqCst), 1);
    assert_eq!(second.starts.load(Ordering::SeqCst), 0);
    drop(first_guard);
    tokio::time::timeout(BOUND, replacement)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.stops.load(Ordering::SeqCst), 1);
    assert_eq!(second.starts.load(Ordering::SeqCst), 1);
    registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn closing_admission_cancels_queued_restart_before_owner_releases_guard() {
    let registry = Arc::new(ChannelRegistry::new());
    let _owner = registry.restart_guard("device").await.unwrap();
    let waiting = registry.clone();
    let queued = tokio::spawn(async move { waiting.restart_guard("device").await.map(|_| ()) });
    registry.close_admission();
    assert!(
        tokio::time::timeout(BOUND, queued)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(registry.restart_guard("device").await.is_err());
    registry.shutdown().await.unwrap();
}

struct PanickingStopChannel {
    retained_resource: Arc<AtomicUsize>,
    stop_entered: Arc<Semaphore>,
}
#[async_trait]
impl ModuleLifecycle for PanickingStopChannel {
    fn name(&self) -> &'static str {
        "panic-stop-channel"
    }
    async fn stop(&self) -> anyhow::Result<()> {
        self.stop_entered.add_permits(1);
        panic!("injected channel stop panic");
    }
}
#[async_trait]
impl ChannelAdapter for PanickingStopChannel {
    fn channel_type(&self) -> &'static str {
        "panic-test"
    }
    fn channel_id(&self) -> &str {
        "panic-test-device"
    }
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities::default()
    }
}
impl Drop for PanickingStopChannel {
    fn drop(&mut self) {
        self.retained_resource.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn panicking_channel_stop_stays_incomplete_and_retains_resource_owner() {
    let registry = ChannelRegistry::new();
    let resource = Arc::new(AtomicUsize::new(0));
    let stop_entered = Arc::new(Semaphore::new(0));
    let channel = Arc::new(PanickingStopChannel {
        retained_resource: resource.clone(),
        stop_entered: stop_entered.clone(),
    });
    let owner = Arc::downgrade(&channel);
    registry.register(channel).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), registry.shutdown())
            .await
            .is_err()
    );
    stop_entered.acquire().await.unwrap().forget();
    // A repeated wait must remain pending, not turn JoinError into a terminal Err.
    assert!(
        tokio::time::timeout(Duration::from_millis(20), registry.shutdown())
            .await
            .is_err()
    );
    assert_eq!(resource.load(Ordering::SeqCst), 0);
    drop(registry);
    tokio::task::yield_now().await;
    assert!(
        owner.upgrade().is_some(),
        "the cleanup task must retain the channel"
    );
    assert_eq!(resource.load(Ordering::SeqCst), 0);
    // The test runtime finally aborts its async tasks; production must retain its
    // runtime/ShutdownHandle while quarantined, as reported by Incomplete.
}
