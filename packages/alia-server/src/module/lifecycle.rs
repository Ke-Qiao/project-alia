use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::ServerConfig;
use crate::event::EventBus;
use crate::expression::ExpressionRuntime;
use crate::plugins::{
    PluginStorage, PluginStorageManifest, PluginStorageService, PluginToolContext,
};
use crate::providers::adapter::{
    CacheBackend, ConversationBackend, EmbeddingAdapter, LlmAdapter, MemoryBackend,
    SchedulerBackend, ToolRegistry,
};
use crate::providers::sensor::SensorRegistry;
use crate::registry::ArtifactRegistry;
use crate::scheduler::{ActionRegistry, ConditionRegistry};
use crate::settings::SettingsRegistry;

/// Health status of a module
#[derive(Debug, Clone)]
pub enum ModuleHealth {
    Healthy,
    Degraded(String),
    Unhealthy(String),
}

/// Shared context provided to modules during initialization (dependency injection)
pub struct ModuleContext {
    pub db: DatabaseConnection,
    pub config: Arc<ServerConfig>,
    pub event_bus: EventBus,
    pub cancel_token: CancellationToken,
    pub settings: SettingsRegistry,
    /// Shared artifact graph for registering ownership/dependencies.
    pub artifacts: Arc<ArtifactRegistry>,
    /// Server-owned plugin-private document storage. Plugin runtimes should
    /// request scoped handles from this service instead of using `db`.
    pub plugin_storage: Arc<PluginStorageService>,
    /// Shared sensor plane. Plugins/components should register and ingest
    /// sensors here so tools, rules, scheduler, and frontend APIs see the same
    /// readings. Plugins that need live tracking can also subscribe to
    /// `event_bus` and watch `ServerEvent::SensorDataIngested`.
    pub sensors: Arc<SensorRegistry>,
    /// Scheduler reference, available for modules initialized after the scheduler.
    pub scheduler: Option<Arc<dyn SchedulerBackend>>,
    /// Condition registry for registering/evaluating task conditions.
    pub conditions: Option<Arc<ConditionRegistry>>,
    /// Action registry shared by scheduled tasks, event rules, and plugins.
    pub actions: Option<Arc<ActionRegistry>>,
    /// Conversation backend for managing sessions (available to channels).
    pub conversation: Option<Arc<dyn ConversationBackend>>,
    /// Memory backend for tools/components initialized after memory.
    pub memory: Option<Arc<dyn MemoryBackend>>,
    /// Default LLM and concrete minor/major handles for modules that run
    /// background reasoning, such as insights.
    pub llm: Option<Arc<dyn LlmAdapter>>,
    pub minor_llm: Option<Arc<dyn LlmAdapter>>,
    pub major_llm: Option<Arc<dyn LlmAdapter>>,
    pub transcript_correction_llm: Option<Arc<dyn LlmAdapter>>,
    pub memory_capture_llm: Option<Arc<dyn LlmAdapter>>,
    pub sentence_expression_llm: Option<Arc<dyn LlmAdapter>>,
    /// Embedding adapter for built-in runtimes that need semantic indexing.
    pub embedding: Option<Arc<dyn EmbeddingAdapter>>,
    /// Cache backend for modules that review or maintain cache entries.
    pub cache: Option<Arc<dyn CacheBackend>>,
    /// Unified executable tool registry. Plugins/components should register
    /// code-backed tool executors here instead of replacing the registry.
    pub tools: Option<Arc<dyn ToolRegistry>>,
    /// Embodied expression runtime. Plugins/components can register appraisal
    /// engines, affect reducers, expression planners, and safety policies here.
    pub expression: Option<Arc<ExpressionRuntime>>,
}

impl ModuleContext {
    pub fn plugin_tool_context(
        &self,
        plugin_name: &str,
        storage_manifest: PluginStorageManifest,
    ) -> anyhow::Result<PluginToolContext> {
        let Some(tools) = self.tools.clone() else {
            anyhow::bail!("tool registry is not available for plugin '{plugin_name}'");
        };
        let storage: Arc<dyn PluginStorage> = Arc::new(
            self.plugin_storage
                .scoped_with_manifest(plugin_name, storage_manifest)?,
        );
        Ok(PluginToolContext {
            plugin_name: plugin_name.to_string(),
            storage,
            tools,
        })
    }
}

/// Base trait for all pluggable modules
#[async_trait]
pub trait ModuleLifecycle: Send + Sync {
    /// Module identifier (used in logs and config references)
    fn name(&self) -> &'static str;

    /// Initialize: receive shared context, establish connections, load resources
    async fn init(&mut self, _ctx: &ModuleContext) -> anyhow::Result<()> {
        Ok(())
    }

    /// Start: called after all modules are initialized, can start background tasks
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Release partial initialization and running resources. Must be idempotent,
    /// and return only after all owned workers exit (including an error result).
    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Health check for /health endpoint aggregation
    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }

    /// Apply runtime setting changes that do not require rebuilding the module.
    ///
    /// Modules should return the dotted setting keys they consumed. Unknown keys
    /// are ignored so plugins and future modules can share one update path.
    async fn apply_settings(
        &self,
        _updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures::FutureExt;
use tokio::sync::watch;
use tokio_util::task::TaskTracker;

/// Admission and completion ownership for request/session tasks, including panics.
#[derive(Default)]
pub struct RuntimeTasks {
    gate: Mutex<bool>,
    tracker: TaskTracker,
    errors: Arc<Mutex<Vec<String>>>,
}

impl RuntimeTasks {
    pub fn try_spawn(
        &self,
        name: &'static str,
        future: impl Future<Output = Result<()>> + Send + 'static,
    ) -> Result<()> {
        let closed = self.gate.lock().expect("runtime task admission mutex");
        anyhow::ensure!(!*closed, "server runtime is stopping");
        let errors = self.errors.clone();
        self.tracker.spawn(async move {
            let result = std::panic::AssertUnwindSafe(future).catch_unwind().await;
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(format!("{name}: {error:#}")),
                Err(_) => Some(format!("{name}: task panicked")),
            };
            if let Some(error) = error {
                errors.lock().expect("runtime task error mutex").push(error);
            }
        });
        Ok(())
    }

    pub fn close(&self) {
        *self.gate.lock().expect("runtime task admission mutex") = true;
        self.tracker.close();
    }

    pub fn active(&self) -> usize {
        self.tracker.len()
    }

    pub async fn wait(&self) -> Result<()> {
        self.tracker.wait().await;
        let errors = self.errors.lock().expect("runtime task error mutex");
        anyhow::ensure!(
            errors.is_empty(),
            "server task failures: {}",
            errors.join("; ")
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
struct ShutdownStatus {
    pending: Vec<String>,
    errors: Vec<String>,
    finished: bool,
}

/// Cleanup continues under its owner when a caller stops waiting.
#[derive(Clone)]
pub struct ShutdownHandle {
    status: watch::Receiver<ShutdownStatus>,
}

impl ShutdownHandle {
    pub fn pending_modules(&self) -> Vec<String> {
        self.status.borrow().pending.clone()
    }

    pub fn is_finished(&self) -> bool {
        self.status.borrow().finished
    }

    pub fn failures(&self) -> Vec<String> {
        self.status.borrow().errors.clone()
    }

    pub async fn wait(&self) -> Result<()> {
        let mut status = self.status.clone();
        loop {
            let current = status.borrow_and_update().clone();
            if current.finished {
                anyhow::ensure!(
                    current.errors.is_empty(),
                    "server shutdown failed: {}",
                    current.errors.join("; ")
                );
                return Ok(());
            }
            status
                .changed()
                .await
                .context("server cleanup owner exited without a result")?;
        }
    }

    pub async fn wait_until(&self, deadline: tokio::time::Instant) -> Result<()> {
        match tokio::time::timeout_at(deadline, self.wait()).await {
            Ok(result) => result,
            Err(_) => Err(ShutdownIncomplete {
                pending_modules: self.pending_modules(),
                handle: self.clone(),
            }
            .into()),
        }
    }
}

#[derive(Clone)]
pub struct ShutdownIncomplete {
    pub pending_modules: Vec<String>,
    pub handle: ShutdownHandle,
}
impl std::fmt::Debug for ShutdownIncomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShutdownIncomplete")
            .field("pending_modules", &self.pending_modules)
            .finish()
    }
}
impl std::fmt::Display for ShutdownIncomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "server shutdown deadline exceeded; cleanup remains owned for {:?}",
            self.pending_modules
        )
    }
}
impl std::error::Error for ShutdownIncomplete {}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ShutdownPhase {
    Ingress,
    Producers,
    Execution,
    Channels,
    Resources,
    Database,
}

type CleanupFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
struct CleanupAction {
    name: String,
    phase: ShutdownPhase,
    // Keep captures independently of the stop future's panic-unwound locals.
    run: Box<dyn Fn() -> CleanupFuture + Send>,
}

pub(crate) struct RuntimeCleanup {
    actions: Mutex<Vec<CleanupAction>>,
    status: watch::Sender<ShutdownStatus>,
    started: std::sync::atomic::AtomicBool,
    cancel: CancellationToken,
    runtime: tokio::runtime::Handle,
    grace: Duration,
}

impl RuntimeCleanup {
    pub(crate) fn new(cancel: CancellationToken, grace: Duration) -> Arc<Self> {
        Arc::new(Self {
            actions: Mutex::new(Vec::new()),
            status: watch::channel(ShutdownStatus::default()).0,
            started: std::sync::atomic::AtomicBool::new(false),
            cancel,
            runtime: tokio::runtime::Handle::current(),
            grace,
        })
    }
    pub(crate) fn record_failure(&self, error: String) {
        self.status.send_modify(|status| status.errors.push(error));
    }
    pub(crate) fn handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            status: self.status.subscribe(),
        }
    }
    pub(crate) fn deadline(&self) -> tokio::time::Instant {
        tokio::time::Instant::now() + self.grace
    }
    pub(crate) fn add<F, Fut>(&self, name: impl Into<String>, phase: ShutdownPhase, action: F)
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let mut actions = self.actions.lock().expect("server cleanup actions mutex");
        assert!(
            !self.started.load(std::sync::atomic::Ordering::Acquire),
            "cannot register resources after cleanup starts"
        );
        actions.push(CleanupAction {
            name: name.into(),
            phase,
            run: Box::new(move || Box::pin(action())),
        });
    }
    pub(crate) fn start(&self) -> ShutdownHandle {
        self.cancel.cancel();
        let mut actions = self.actions.lock().expect("server cleanup actions mutex");
        if self.started.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return self.handle();
        }
        let mut actions = std::mem::take(&mut *actions);
        // Reverse registration within each dependency phase, never put outbound
        // resources or the pool ahead of a still-running Execution worker.
        actions.reverse();
        actions.sort_by_key(|action| action.phase);
        self.status.send_modify(|status| {
            status.pending = actions.iter().map(|action| action.name.clone()).collect()
        });
        let status = self.status.clone();
        let runtime = self.runtime.clone();
        // A native stop may synchronously join an OS thread. Keep the owner off
        // the host executor, so a current-thread host can still observe deadlines.
        self.runtime.spawn_blocking(move || {
            for action in actions {
                let name = &action.name;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on((action.run)())
                }));
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(format!("{name}: {error:#}")),
                    Err(_) => {
                        let error = format!("{name}: cleanup panicked; completion is unproven");
                        tracing::error!(%error, "server cleanup cannot release dependencies");
                        status.send_modify(|state| state.errors.push(error));
                        // Never retry a panicked stop or drop its captured owner.
                        // The remaining actions retain downstream resources.
                        // The host receives Incomplete and may decide to exit, but
                        // the library cannot safely pretend this module has joined.
                        runtime.block_on(async move {
                            let _retained = action;
                            std::future::pending::<()>().await;
                        });
                        unreachable!("unproven cleanup deliberately remains pending");
                    }
                };
                if let Some(error) = &error {
                    tracing::error!(%error, "server cleanup failed");
                }
                status.send_modify(|state| {
                    if let Some(error) = error {
                        state.errors.push(error);
                    }
                    state.pending.remove(0);
                });
            }
            status.send_modify(|state| state.finished = true);
        });
        self.handle()
    }

    /// The rollback slot exists before init is polled, including partial failures.
    pub(crate) async fn initialize<T: ModuleLifecycle + ?Sized + 'static>(
        &self,
        module: Box<T>,
        ctx: &ModuleContext,
        phase: ShutdownPhase,
    ) -> Result<Arc<T>> {
        self.initialize_owned(module, ctx, phase)
            .await
            .map(|(module, _)| module)
    }

    pub(crate) async fn initialize_owned<T: ModuleLifecycle + ?Sized + 'static>(
        &self,
        module: Box<T>,
        ctx: &ModuleContext,
        phase: ShutdownPhase,
    ) -> Result<(Arc<T>, Arc<std::sync::atomic::AtomicBool>)> {
        enum Slot<T: ?Sized> {
            Initializing(Box<T>),
            Ready(Arc<T>),
            Moving,
        }
        let name = module.name();
        let slot = Arc::new(tokio::sync::Mutex::new(Slot::Initializing(module)));
        let rollback = slot.clone();
        let transferred = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let skip = transferred.clone();
        self.add(name, phase, move || {
            let skip = skip.clone();
            let rollback = rollback.clone();
            async move {
                if skip.load(std::sync::atomic::Ordering::Acquire) {
                    return Ok(());
                }
                let module = rollback.lock().await;
                match &*module {
                    Slot::Initializing(module) => module.stop().await,
                    Slot::Ready(module) => module.stop().await,
                    Slot::Moving => Err(anyhow!("module ownership transition interrupted")),
                }
            }
        });
        let mut slot = slot.lock().await;
        let Slot::Initializing(module) = &mut *slot else {
            unreachable!()
        };
        startup_step(
            &ctx.cancel_token,
            &format!("initializing module {name}"),
            module.init(ctx),
        )
        .await?;
        // No await occurs during this move; the mutex keeps cleanup out until
        // the ready Arc is back in the same registered ownership slot.
        let ready: Arc<T> = match std::mem::replace(&mut *slot, Slot::Moving) {
            Slot::Initializing(module) => Arc::from(module),
            Slot::Ready(_) | Slot::Moving => unreachable!(),
        };
        *slot = Slot::Ready(ready.clone());
        Ok((ready, transferred))
    }
}

pub(crate) async fn startup_step<T>(
    cancel: &CancellationToken,
    name: &str,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(anyhow!("server startup cancelled while {name}")),
        result = std::panic::AssertUnwindSafe(future).catch_unwind() =>
            result.unwrap_or_else(|_| Err(anyhow!("startup panicked"))).with_context(|| name.to_string()),
    }
}

pub(crate) async fn start_module(
    module: &dyn ModuleLifecycle,
    cancel: &CancellationToken,
) -> Result<()> {
    startup_step(
        cancel,
        &format!("starting module {}", module.name()),
        module.start(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // A panicked, unproven stop intentionally remains owned. Isolate that case
    // so the test runner itself does not retain an immortal blocking worker.
    #[test]
    fn panicked_stop_retains_owner_and_dependencies_as_incomplete() {
        const CHILD: &str = "ALIA_RUNTIME_PANIC_CLEANUP_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "module::lifecycle::tests::panicked_stop_retains_owner_and_dependencies_as_incomplete", "--nocapture"])
                .env(CHILD, "1")
                .spawn().unwrap();
            let until = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success(), "isolated panic cleanup contract failed");
                    break;
                }
                if std::time::Instant::now() >= until {
                    child.kill().unwrap();
                    let _ = child.wait();
                    panic!("panic cleanup child exceeded its process deadline");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            return;
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let checked = runtime.block_on(async {
            let owner = Arc::new(AtomicUsize::new(0));
            let weak = Arc::downgrade(&owner);
            let dependent_stops = Arc::new(AtomicUsize::new(0));
            let cleanup = RuntimeCleanup::new(CancellationToken::new(), Duration::from_millis(30));
            let dependent = dependent_stops.clone();
            cleanup.add("dependent_database", ShutdownPhase::Database, move || {
                let dependent = dependent.clone();
                async move {
                    dependent.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            });
            cleanup.add("panicking_module", ShutdownPhase::Resources, move || {
                let polled_owner = owner.clone();
                async move {
                    polled_owner.fetch_add(1, Ordering::SeqCst);
                    panic!("injected stop panic");
                }
            });
            let handle = cleanup.start();
            let error = handle.wait_until(cleanup.deadline()).await.unwrap_err();
            assert!(error.is::<ShutdownIncomplete>());
            assert!(!handle.is_finished());
            assert_eq!(
                handle.pending_modules(),
                ["panicking_module", "dependent_database"]
            );
            assert!(
                handle
                    .failures()
                    .iter()
                    .any(|error| error.contains("completion is unproven"))
            );
            assert_eq!(dependent_stops.load(Ordering::SeqCst), 0);
            drop(cleanup);
            assert_eq!(
                weak.upgrade()
                    .expect("panic owner must remain alive")
                    .load(Ordering::SeqCst),
                1
            );
            true
        });
        runtime.shutdown_timeout(Duration::from_millis(30));
        assert!(checked);
    }
}
