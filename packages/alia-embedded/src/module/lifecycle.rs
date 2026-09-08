use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use futures::FutureExt;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Health status of a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleHealth {
    Healthy,
    Degraded,
    Unhealthy,
}

/// A module must release partially initialized resources in `stop` as well as
/// running resources. Stop must be idempotent and finish only after workers exit.
#[tonic::async_trait]
pub trait ModuleLifecycle: Send + Sync {
    fn name(&self) -> &'static str;
    async fn init(&mut self, ctx: &super::context::ModuleContext) -> Result<()> {
        let _ = ctx;
        Ok(())
    }
    async fn start(&self) -> Result<()> {
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        Ok(())
    }
    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }
}

#[derive(Clone, Debug, Default)]
struct ShutdownStatus {
    pending: Vec<&'static str>,
    errors: Vec<String>,
    finished: bool,
}

/// Retains cleanup progress after a caller's shutdown deadline. A timeout never
/// means a native worker has stopped; hosts may continue awaiting this handle.
#[derive(Clone)]
pub struct ShutdownHandle {
    status: watch::Receiver<ShutdownStatus>,
}

impl ShutdownHandle {
    pub fn pending_modules(&self) -> Vec<&'static str> {
        self.status.borrow().pending.clone()
    }

    pub async fn wait(&self) -> Result<()> {
        let mut status = self.status.clone();
        loop {
            let current = status.borrow_and_update().clone();
            if current.finished {
                if current.errors.is_empty() {
                    return Ok(());
                }
                bail!("module shutdown failed: {}", current.errors.join("; "));
            }
            status
                .changed()
                .await
                .context("cleanup supervisor exited without a result")?;
        }
    }

    pub(crate) async fn wait_until(&self, deadline: tokio::time::Instant) -> Result<()> {
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

/// Cleanup is still owned by the supervisor. This error exposes a handle so a
/// library host can await actual completion instead of mistaking timeout for exit.
#[derive(Clone)]
pub struct ShutdownIncomplete {
    /// Snapshot at the deadline; the handle exposes subsequent progress.
    pub pending_modules: Vec<&'static str>,
    pub handle: ShutdownHandle,
}

impl std::fmt::Debug for ShutdownIncomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShutdownIncomplete")
            .field("pending", &self.pending_modules)
            .finish()
    }
}
impl std::fmt::Display for ShutdownIncomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "shutdown deadline exceeded; cleanup still running for {:?}",
            self.pending_modules
        )
    }
}
impl std::error::Error for ShutdownIncomplete {}

/// One owner for the initialized module stack, including partial init failures.
/// Reverse construction order is also reverse dependency order.
pub(crate) struct ModuleCleanup {
    modules: Mutex<Vec<Arc<dyn ModuleLifecycle>>>,
    status: watch::Sender<ShutdownStatus>,
    cancel: CancellationToken,
    started: std::sync::atomic::AtomicBool,
    runtime: tokio::runtime::Handle,
    grace: std::time::Duration,
}

impl ModuleCleanup {
    pub(crate) fn new(cancel: CancellationToken, grace: std::time::Duration) -> Arc<Self> {
        Arc::new(Self {
            modules: Mutex::new(Vec::new()),
            status: watch::channel(ShutdownStatus::default()).0,
            cancel,
            started: std::sync::atomic::AtomicBool::new(false),
            runtime: tokio::runtime::Handle::current(),
            grace,
        })
    }

    pub(crate) fn deadline(&self) -> tokio::time::Instant {
        tokio::time::Instant::now() + self.grace
    }

    pub(crate) fn add(&self, module: Arc<dyn ModuleLifecycle>) {
        self.modules
            .lock()
            .expect("cleanup module mutex")
            .push(module);
    }

    pub(crate) fn handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            status: self.status.subscribe(),
        }
    }

    pub(crate) fn start(&self) -> ShutdownHandle {
        use std::sync::atomic::Ordering;
        self.cancel.cancel();
        if self.started.swap(true, Ordering::AcqRel) {
            return self.handle();
        }
        let modules = std::mem::take(&mut *self.modules.lock().expect("cleanup module mutex"));
        let status = self.status.clone();
        status
            .send_modify(|state| state.pending = modules.iter().rev().map(|m| m.name()).collect());
        let runtime = self.runtime.clone();
        // Native audio/sensor stop currently joins OS threads synchronously.
        // A single owned blocking worker keeps that join off library executors,
        // including current-thread Tokio hosts. It remains owned after timeout.
        self.runtime.spawn_blocking(move || {
            for module in modules.into_iter().rev() {
                let name = module.name();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(module.stop())
                }));
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(format!("{name}: {error:#}")),
                    Err(_) => Some(format!("{name}: stop panicked")),
                };
                if let Some(error) = &error {
                    tracing::error!(%error, "module shutdown failed");
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
}

/// Dropping an initialization/run future requests shutdown without abandoning
/// the internal task that still owns partially initialized modules.
pub(crate) struct CancelOnDrop(pub Option<CancellationToken>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(cancel) = &self.0 {
            cancel.cancel();
        }
    }
}

pub(crate) async fn initialize<T: ModuleLifecycle + ?Sized>(
    mut module: Box<T>,
    ctx: &super::context::ModuleContext,
) -> (Arc<T>, Result<()>) {
    let name = module.name();
    let result = tokio::select! {
        biased;
        _ = ctx.cancel_token.cancelled() => Err(anyhow!("startup cancelled while initializing {name}")),
        result = std::panic::AssertUnwindSafe(module.init(ctx)).catch_unwind() =>
            result.unwrap_or_else(|_| Err(anyhow!("init panicked"))).with_context(|| format!("initializing module {name}")),
    };
    (Arc::from(module), result)
}

pub(crate) async fn start_module(
    module: &dyn ModuleLifecycle,
    cancel: &CancellationToken,
) -> Result<()> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("startup cancelled while starting {}", module.name()),
        result = module.start() => result.with_context(|| format!("starting module {}", module.name())),
    }
}

/// Use for cancellable infrastructure work after its resource owner is registered.
pub(crate) async fn startup_step<T>(
    cancel: &CancellationToken,
    name: &str,
    work: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("startup cancelled while {name}"),
        result = work => result.with_context(|| name.to_string()),
    }
}
