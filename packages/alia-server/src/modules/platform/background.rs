//! One-shot ownership of a module's cooperative background tasks.
//!
//! Cancellation interrupts async endpoint waits. Native work must retain its own
//! cleanup handle; dropping an async future does not stop a native thread.
use std::future::Future;

use tokio::sync::{Mutex, MutexGuard};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct OwnedBackground {
    cancel: CancellationToken,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    started: bool,
    tasks: Vec<(&'static str, Option<JoinHandle<anyhow::Result<()>>>)>,
    failures: Vec<String>,
}

pub struct BackgroundStart<'a> {
    state: MutexGuard<'a, State>,
    cancel: CancellationToken,
    committed: bool,
}

impl OwnedBackground {
    /// Holds the start gate across preparation so concurrent start cannot
    /// create duplicate workers. Dropping an incomplete start cancels its tasks.
    pub async fn begin_start(
        &self,
        parent: &CancellationToken,
    ) -> anyhow::Result<BackgroundStart<'_>> {
        let mut state = self.state.lock().await;
        anyhow::ensure!(
            !self.cancel.is_cancelled() && !parent.is_cancelled(),
            "background is stopping"
        );
        anyhow::ensure!(
            !state.started,
            "background start has already been attempted"
        );
        state.started = true;
        let cancel = self.cancel.clone();
        let parent = parent.clone();
        state.tasks.push((
            "parent cancellation",
            Some(tokio::spawn(async move {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {},
                    _ = parent.cancelled() => cancel.cancel(),
                }
                Ok(())
            })),
        ));
        Ok(BackgroundStart {
            state,
            cancel: self.cancel.clone(),
            committed: false,
        })
    }

    pub fn request_stop(&self) {
        self.cancel.cancel();
    }

    pub async fn cancelled(&self) {
        self.cancel.cancelled().await;
    }

    pub fn is_stopping(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub async fn stop(&self) -> anyhow::Result<()> {
        self.request_stop();
        let mut state = self.state.lock().await;
        for index in 0..state.tasks.len() {
            let (name, task) = &mut state.tasks[index];
            let Some(handle) = task.as_mut() else {
                continue;
            };
            // Keep the handle stored while awaiting. A cancelled stop waiter
            // cannot detach the remaining tasks or consume their result.
            let result = handle.await;
            task.take();
            if let Err(error) = result
                .map_err(anyhow::Error::from)
                .and_then(|result| result)
            {
                let error = format!("{name}: {error:#}");
                state.failures.push(error);
            }
        }
        anyhow::ensure!(
            state.failures.is_empty(),
            "background shutdown failed: {}",
            state.failures.join("; ")
        );
        Ok(())
    }
}

impl BackgroundStart<'_> {
    /// For jobs that own native cleanup: the job must observe token(), finish
    /// cleanup itself and return. Cancellation must not drop this future.
    pub fn spawn_owned(
        &mut self,
        name: &'static str,
        job: impl Future<Output = anyhow::Result<()>> + Send + 'static,
    ) {
        self.state.tasks.push((name, Some(tokio::spawn(job))));
    }

    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn spawn_loop(
        &mut self,
        name: &'static str,
        job: impl Future<Output = ()> + Send + 'static,
    ) {
        self.spawn(name, async move {
            job.await;
            Ok(())
        });
    }

    /// Jobs must be cancellation-safe async loops. Local cancellation wins even
    /// when the loop is currently awaiting an endpoint inside its select branch.
    pub fn spawn(
        &mut self,
        name: &'static str,
        job: impl Future<Output = anyhow::Result<()>> + Send + 'static,
    ) {
        let cancel = self.cancel.clone();
        self.state.tasks.push((
            name,
            Some(tokio::spawn(async move {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Ok(()),
                    result = job => {
                        if result.is_ok() && !cancel.is_cancelled() {
                            anyhow::bail!("background task exited before cancellation");
                        }
                        result
                    },
                }
            })),
        ));
    }

    pub fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for BackgroundStart<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.cancel.cancel();
        }
    }
}

impl Drop for OwnedBackground {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
