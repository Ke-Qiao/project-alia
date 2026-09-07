use std::collections::HashMap;
use std::future::Future;

use tokio::task::{Id, JoinSet};
use tokio_util::sync::CancellationToken;

pub(super) const MAX_CONCURRENT_REQUESTS: usize = 4;
pub(super) const LEGACY_QUEUE_CAPACITY: usize = 8;

/// Owns every running request, including those draining dispatched tools.
#[derive(Default)]
pub(super) struct PipelineTasks {
    tasks: JoinSet<()>,
    tokens: HashMap<Id, CancellationToken>,
}

impl PipelineTasks {
    pub(super) fn has_capacity(&self) -> bool {
        self.tasks.len() < MAX_CONCURRENT_REQUESTS
    }

    pub(super) fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub(super) fn spawn(
        &mut self,
        token: CancellationToken,
        work: impl Future<Output = ()> + Send + 'static,
    ) {
        assert!(
            self.has_capacity(),
            "pipeline admission exceeded request limit"
        );
        let check = token.clone();
        let handle = self.tasks.spawn(async move {
            if !check.is_cancelled() {
                work.await;
            }
        });
        self.tokens.insert(handle.id(), token);
    }

    pub(super) async fn join_next(&mut self) {
        match self.tasks.join_next_with_id().await {
            Some(Ok((id, ()))) => {
                self.tokens.remove(&id);
            }
            Some(Err(error)) => {
                self.tokens.remove(&error.id());
                tracing::error!(error = %error, "pipeline request task failed");
            }
            None => {}
        }
    }

    pub(super) async fn shutdown(mut self) {
        for token in self.tokens.values() {
            token.cancel();
        }
        while !self.tasks.is_empty() {
            self.join_next().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn admission_is_bounded_until_completed_tasks_are_joined() {
        let mut tasks = PipelineTasks::default();
        for _ in 0..MAX_CONCURRENT_REQUESTS {
            let token = CancellationToken::new();
            let worker_token = token.clone();
            tasks.spawn(token, async move {
                worker_token.cancelled().await;
            });
        }
        assert!(!tasks.has_capacity());
        tasks.shutdown().await;
    }

    #[tokio::test]
    async fn cancelled_admission_never_polls_request() {
        let mut tasks = PipelineTasks::default();
        let token = CancellationToken::new();
        let polled = Arc::new(AtomicBool::new(false));
        let marker = polled.clone();
        token.cancel();
        tasks.spawn(token, async move {
            marker.store(true, Ordering::SeqCst);
        });
        tasks.join_next().await;
        assert!(!polled.load(Ordering::SeqCst));
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn shutdown_waits_for_dispatched_effect_fact_after_cancellation() {
        let mut tasks = PipelineTasks::default();
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let (started_tx, started_rx) = oneshot::channel();
        let (complete_tx, complete_rx) = oneshot::channel();
        let recorded = Arc::new(AtomicBool::new(false));
        let marker = recorded.clone();
        tasks.spawn(token.clone(), async move {
            started_tx.send(()).unwrap();
            // Models a dispatched tool; its completion/trace must survive a
            // cancelled response and must remain supervised until finished.
            complete_rx.await.unwrap();
            assert!(worker_token.is_cancelled());
            marker.store(true, Ordering::SeqCst);
        });
        started_rx.await.unwrap();
        let mut shutdown = Box::pin(tasks.shutdown());
        tokio::select! {
            biased;
            _ = &mut shutdown => panic!("shutdown lost in-flight effect"),
            _ = token.cancelled() => {}
        }
        assert!(!recorded.load(Ordering::SeqCst));
        complete_tx.send(()).unwrap();
        shutdown.await;
        assert!(recorded.load(Ordering::SeqCst));
    }
}
