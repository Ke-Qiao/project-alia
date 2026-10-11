use super::*;
use async_trait::async_trait;
use futures::poll;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;

#[derive(Default)]
struct Counts {
    active: AtomicUsize,
    maximum: AtomicUsize,
    released: AtomicUsize,
}

struct ActiveProbe(Arc<Counts>);
impl Drop for ActiveProbe {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.released.fetch_add(1, Ordering::SeqCst);
    }
}

enum Behavior {
    Hold,
    FinishAfter(Duration, ModuleHealth),
    Panic,
}

struct Probe {
    calls: AtomicUsize,
    counts: Arc<Counts>,
    release: Semaphore,
    behavior: Behavior,
}
impl Probe {
    fn new(counts: &Arc<Counts>, behavior: Behavior) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            counts: counts.clone(),
            release: Semaphore::new(0),
            behavior,
        }
    }
}
#[async_trait]
impl ModuleLifecycle for Probe {
    fn name(&self) -> &'static str {
        "same-provider-name"
    }
    async fn health_check(&self) -> ModuleHealth {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.counts.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.counts.maximum.fetch_max(active, Ordering::SeqCst);
        let _active = ActiveProbe(self.counts.clone());
        match &self.behavior {
            Behavior::Hold => {
                self.release.acquire().await.unwrap().forget();
                ModuleHealth::Healthy
            }
            Behavior::FinishAfter(delay, health) => {
                if !delay.is_zero() {
                    tokio::time::sleep(*delay).await;
                }
                health.clone()
            }
            Behavior::Panic => panic!("injected probe panic"),
        }
    }
}

fn assert_degraded(health: &ModuleHealth, expected: &str) {
    assert!(
        matches!(health, ModuleHealth::Degraded(message) if message == expected),
        "{health:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn aliases_share_one_probe_but_same_names_do_not_merge_distinct_instances() {
    let counts = Arc::new(Counts::default());
    let shared = Arc::new(Probe::new(
        &counts,
        Behavior::FinishAfter(Duration::ZERO, ModuleHealth::Healthy),
    ));
    let alias = shared.clone();
    let different = Probe::new(
        &counts,
        Behavior::FinishAfter(
            Duration::ZERO,
            ModuleHealth::Degraded("distinct instance".into()),
        ),
    );
    let panic = Probe::new(&counts, Behavior::Panic);
    let modules: Vec<(&str, &dyn ModuleLifecycle)> = vec![
        ("llm.minor", shared.as_ref()),
        ("llm.major", alias.as_ref()),
        ("llm.memory_capture", &different),
        ("stt", &panic),
    ];
    let result = aggregate_health(&modules, &Mutex::new(()), &CancellationToken::new()).await;
    assert_eq!(result.len(), 4);
    assert_eq!(shared.calls.load(Ordering::SeqCst), 1);
    assert_eq!(different.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(result["llm.minor"], ModuleHealth::Healthy));
    assert!(matches!(result["llm.major"], ModuleHealth::Healthy));
    assert_degraded(&result["llm.memory_capture"], "distinct instance");
    assert!(
        matches!(&result["stt"], ModuleHealth::Unhealthy(message) if message == "health check panicked")
    );
    assert_eq!(counts.active.load(Ordering::SeqCst), 0);
    assert_eq!(counts.released.load(Ordering::SeqCst), 3);
}

#[tokio::test(start_paused = true)]
async fn total_deadline_keeps_finished_results_and_releases_every_running_probe() {
    let counts = Arc::new(Counts::default());
    let healthy = Probe::new(
        &counts,
        Behavior::FinishAfter(Duration::from_secs(1), ModuleHealth::Healthy),
    );
    let bad = Probe::new(
        &counts,
        Behavior::FinishAfter(
            Duration::from_secs(2),
            ModuleHealth::Unhealthy("known failure".into()),
        ),
    );
    let stalled = Probe::new(&counts, Behavior::Hold);
    let later = Probe::new(
        &counts,
        Behavior::FinishAfter(Duration::from_secs(30), ModuleHealth::Healthy),
    );
    let modules: Vec<(&str, &dyn ModuleLifecycle)> = vec![
        ("healthy", &healthy),
        ("bad", &bad),
        ("stalled", &stalled),
        ("later", &later),
    ];
    let gate = Mutex::new(());
    let started = tokio::time::Instant::now();
    let result = aggregate_health(&modules, &gate, &CancellationToken::new()).await;
    assert_eq!(started.elapsed(), HEALTH_BUDGET);
    assert_eq!(result.len(), 4);
    assert!(matches!(result["healthy"], ModuleHealth::Healthy));
    assert!(
        matches!(&result["bad"], ModuleHealth::Unhealthy(message) if message == "known failure")
    );
    for name in ["stalled", "later"] {
        assert_degraded(&result[name], "health check timed out after 6s");
    }
    assert_eq!(counts.active.load(Ordering::SeqCst), 0);
    assert_eq!(counts.released.load(Ordering::SeqCst), 4);
    assert!(gate.try_lock().is_ok());
}

#[tokio::test(start_paused = true)]
async fn concurrency_limit_also_bounds_queued_checks_by_the_original_deadline() {
    let counts = Arc::new(Counts::default());
    let probes = (0..10)
        .map(|_| Probe::new(&counts, Behavior::Hold))
        .collect::<Vec<_>>();
    let names = (0..10).map(|i| format!("module.{i}")).collect::<Vec<_>>();
    let modules = names
        .iter()
        .zip(&probes)
        .map(|(name, probe)| (name.as_str(), probe as &dyn ModuleLifecycle))
        .collect::<Vec<_>>();
    let gate = Mutex::new(());
    let cancel = CancellationToken::new();
    let mut check = Box::pin(aggregate_health(&modules, &gate, &cancel));
    assert!(poll!(check.as_mut()).is_pending());
    assert_eq!(counts.active.load(Ordering::SeqCst), HEALTH_CONCURRENCY);
    assert_eq!(probes[8].calls.load(Ordering::SeqCst), 0);
    assert_eq!(probes[9].calls.load(Ordering::SeqCst), 0);
    tokio::time::advance(Duration::from_secs(4)).await;
    probes[0].release.add_permits(1);
    assert!(poll!(check.as_mut()).is_pending());
    assert_eq!(probes[8].calls.load(Ordering::SeqCst), 1);
    assert_eq!(probes[9].calls.load(Ordering::SeqCst), 0);
    tokio::time::advance(Duration::from_secs(2)).await;
    let result = check.await;
    assert!(matches!(result["module.0"], ModuleHealth::Healthy));
    for name in &names[1..] {
        assert_degraded(&result[name], "health check timed out after 6s");
    }
    assert_eq!(counts.maximum.load(Ordering::SeqCst), HEALTH_CONCURRENCY);
    assert_eq!(counts.active.load(Ordering::SeqCst), 0);
    assert_eq!(counts.released.load(Ordering::SeqCst), 9);
    assert_eq!(probes[9].calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn busy_requests_do_not_queue_and_dropped_callers_release_admission() {
    let counts = Arc::new(Counts::default());
    let probe = Probe::new(&counts, Behavior::Hold);
    let modules: Vec<(&str, &dyn ModuleLifecycle)> = vec![("minor", &probe), ("major", &probe)];
    let gate = Mutex::new(());
    let cancel = CancellationToken::new();
    let mut first = Box::pin(aggregate_health(&modules, &gate, &cancel));
    assert!(poll!(first.as_mut()).is_pending());
    for _ in 0..4 {
        let result = aggregate_health(&modules, &gate, &cancel).await;
        assert_eq!(result.len(), 2);
        for health in result.values() {
            assert_degraded(health, "health check already in progress");
        }
    }
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(counts.active.load(Ordering::SeqCst), 1);
    drop(first);
    assert_eq!(counts.active.load(Ordering::SeqCst), 0);
    assert_eq!(counts.released.load(Ordering::SeqCst), 1);
    probe.release.add_permits(1);
    let next = aggregate_health(&modules, &gate, &cancel).await;
    assert!(
        next.values()
            .all(|health| matches!(health, ModuleHealth::Healthy))
    );
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn stopping_releases_pending_checks_and_rejects_future_probe_admission() {
    let counts = Arc::new(Counts::default());
    let ready = Probe::new(
        &counts,
        Behavior::FinishAfter(Duration::ZERO, ModuleHealth::Healthy),
    );
    let held = Probe::new(&counts, Behavior::Hold);
    let modules: Vec<(&str, &dyn ModuleLifecycle)> = vec![("ready", &ready), ("held", &held)];
    let gate = Mutex::new(());
    let cancel = CancellationToken::new();
    let mut check = Box::pin(aggregate_health(&modules, &gate, &cancel));
    assert!(poll!(check.as_mut()).is_pending());
    cancel.cancel();
    let result = check.await;
    assert!(matches!(result["ready"], ModuleHealth::Healthy));
    assert_degraded(&result["held"], "server is stopping");
    assert_eq!(counts.active.load(Ordering::SeqCst), 0);
    assert_eq!(counts.released.load(Ordering::SeqCst), 2);
    let next = aggregate_health(&modules, &gate, &cancel).await;
    for health in next.values() {
        assert_degraded(health, "server is stopping");
    }
    assert_eq!(ready.calls.load(Ordering::SeqCst), 1);
    assert_eq!(held.calls.load(Ordering::SeqCst), 1);
    assert!(gate.try_lock().is_ok());
}
