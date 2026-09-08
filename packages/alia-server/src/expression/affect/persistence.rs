//! One bounded writer owned by the expression instance. Closing admission and
//! draining accepted writes precede release of the runtime's database pool.
use std::sync::Mutex;
use std::time::Duration;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};
use tokio::task::JoinHandle;

use super::{
    AffectPolicySection, AppraisalEvent, delete_persisted_relationship_appraisals,
    insert_appraisals,
};

const CAPACITY: usize = 128;
const MAX_FAILURES: usize = 32;
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) struct AppraisalWrite {
    pub channel_id: String,
    pub user_id: String,
    pub route: Option<String>,
    pub appraisals: Vec<AppraisalEvent>,
}

pub(super) type ClearAcknowledgement = oneshot::Receiver<Result<u64, String>>;

enum QueueCommand {
    Write(AppraisalWrite),
    ClearRelationship {
        user_id: String,
        reply: oneshot::Sender<Result<u64, String>>,
    },
}

#[derive(Debug, Default)]
struct Admission {
    started: bool,
    closed: bool,
    sender: Option<mpsc::Sender<QueueCommand>>,
    failures: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct Persistence {
    admission: Mutex<Admission>,
    task: AsyncMutex<Option<JoinHandle<Vec<String>>>>,
}

fn remember(failures: &mut Vec<String>, error: String) {
    if failures.len() < MAX_FAILURES {
        failures.push(error);
    }
}

impl Persistence {
    pub async fn start(
        &self,
        db: DatabaseConnection,
        policy: AffectPolicySection,
    ) -> anyhow::Result<()> {
        // Keep the sole join slot before creating any worker, including during
        // startup cancellation. This is a one-shot instance, never a restart.
        let mut task = self.task.lock().await;
        let mut admission = self
            .admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        anyhow::ensure!(
            !admission.started && !admission.closed,
            "expression persistence already started or closed"
        );
        let (sender, mut receiver) = mpsc::channel::<QueueCommand>(CAPACITY);
        admission.started = true;
        admission.sender = Some(sender);
        *task = Some(tokio::spawn(async move {
            let mut failures = Vec::new();
            while let Some(command) = receiver.recv().await {
                let (user_id, write, reply) = match command {
                    QueueCommand::Write(write) => (write.user_id.clone(), Some(write), None),
                    QueueCommand::ClearRelationship { user_id, reply } => {
                        (user_id, None, Some(reply))
                    }
                };
                let result = tokio::time::timeout(WRITE_TIMEOUT, async {
                    let tx = db.begin().await?;
                    // If a SQL acknowledgement is lost, dropping its Rust future
                    // alone does not prove PostgreSQL stopped executing it. The
                    // next command must await the previous transaction's lock.
                    tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                        "SELECT pg_advisory_xact_lock(hashtext('alia.expression.persistence'),hashtext($1))", [user_id.clone().into()])).await?;
                    let removed = if let Some(write) = write {
                        insert_appraisals(&tx, write.channel_id, write.user_id, write.route, write.appraisals, policy.clone()).await?;
                        0
                    } else {
                        delete_persisted_relationship_appraisals(&tx, &user_id).await?
                    };
                    tx.commit().await?;
                    Ok::<_,anyhow::Error>(removed)
                }).await.map_err(|_| "appraisal commit acknowledgement timed out; outcome may be unknown".to_string())
                    .and_then(|result| result.map_err(|error| format!("appraisal persistence failed: {error:#}")));
                if let Err(error) = &result {
                    tracing::error!(error = %error, "expression persistence could not confirm write");
                    remember(&mut failures, error.clone());
                }
                if let Some(reply) = reply {
                    // Once admitted, deletion belongs to this writer even when
                    // the API caller times out or drops its acknowledgement.
                    let _ = reply.send(result);
                }
            }
            failures
        }));
        Ok(())
    }

    pub fn enqueue(&self, write: AppraisalWrite) -> anyhow::Result<()> {
        self.enqueue_command(QueueCommand::Write(write))
    }

    pub fn clear_relationship(&self, user_id: &str) -> anyhow::Result<ClearAcknowledgement> {
        let (reply, acknowledged) = oneshot::channel();
        self.enqueue_command(QueueCommand::ClearRelationship {
            user_id: user_id.into(),
            reply,
        })?;
        Ok(acknowledged)
    }

    fn enqueue_command(&self, command: QueueCommand) -> anyhow::Result<()> {
        let mut admission = self
            .admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let result = match admission.sender.as_ref() {
            Some(sender) if !admission.closed => {
                sender.try_send(command).map_err(|error| match error {
                    mpsc::error::TrySendError::Full(_) => "expression persistence queue is full",
                    mpsc::error::TrySendError::Closed(_) => {
                        "expression persistence worker has stopped"
                    }
                })
            }
            _ => Err("expression persistence admission is closed"),
        };
        if let Err(error) = result {
            remember(&mut admission.failures, error.to_string());
            anyhow::bail!(error);
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        {
            // This is the same gate as enqueue: nothing can be admitted after
            // shutdown seals it, while queued writes remain owned by the worker.
            let mut admission = self
                .admission
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            admission.closed = true;
            admission.sender.take();
        }
        let mut task = self.task.lock().await;
        if let Some(handle) = task.as_mut() {
            let result = handle.await;
            task.take();
            let mut admission = self
                .admission
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match result {
                Ok(failures) => {
                    for error in failures {
                        remember(&mut admission.failures, error);
                    }
                }
                Err(error) => remember(
                    &mut admission.failures,
                    format!("expression persistence worker failed: {error}"),
                ),
            }
        }
        let admission = self
            .admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        anyhow::ensure!(
            admission.failures.is_empty(),
            "expression persistence shutdown failed: {}",
            admission.failures.join("; ")
        );
        Ok(())
    }
}

impl Drop for Persistence {
    fn drop(&mut self) {
        let admission = self
            .admission
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        admission.closed = true;
        admission.sender.take();
    }
}
