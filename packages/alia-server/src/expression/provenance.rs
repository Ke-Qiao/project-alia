//! One supervised writer owns admitted canonical appraisal mutations. Waiting
//! for an acknowledgement is cancellable; ownership of the mutation is not.
use std::sync::Mutex;
use std::time::Duration;

use sea_orm::{ConnectionTrait, DatabaseConnection, TransactionTrait};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};
use tokio::task::JoinHandle;

use super::AppraisalEvent;
use crate::modules::memory::context::{AppraisalInput, AppraisalReceipt};

struct Write {
    input: AppraisalInput,
    drafts: Vec<AppraisalEvent>,
    reply: oneshot::Sender<anyhow::Result<AppraisalReceipt>>,
}

#[derive(Default)]
struct Admission {
    started: bool,
    closed: bool,
    sender: Option<mpsc::Sender<Write>>,
    failures: Vec<String>,
}

#[derive(Default)]
pub(crate) struct AppraisalWriter {
    admission: Mutex<Admission>,
    task: AsyncMutex<Option<JoinHandle<Vec<String>>>>,
}
impl std::fmt::Debug for AppraisalWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let gate = self.admission.lock().unwrap_or_else(|e| e.into_inner());
        f.debug_struct("AppraisalWriter")
            .field("started", &gate.started)
            .field("closed", &gate.closed)
            .finish()
    }
}
impl AppraisalWriter {
    pub async fn start(&self, db: DatabaseConnection) -> anyhow::Result<()> {
        let mut task = self.task.lock().await;
        let mut gate = self.admission.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            !gate.started && !gate.closed,
            "appraisal writer already started or closed"
        );
        let (sender, mut receiver) = mpsc::channel::<Write>(128);
        gate.started = true;
        gate.sender = Some(sender);
        *task = Some(tokio::spawn(async move {
            let mut failures = Vec::new();
            while let Some(write) = receiver.recv().await {
                let id = write.input.id();
                let result = tokio::time::timeout(Duration::from_secs(2), async {
                    let tx = db.begin().await?;
                    tx.execute(sea_orm::Statement::from_string(
                        sea_orm::DatabaseBackend::Postgres,
                        "SET LOCAL statement_timeout='2s'".to_owned(),
                    ))
                    .await?;
                    let receipt = crate::modules::memory::context::appraisal::submit(
                        &tx,
                        write.input,
                        write.drafts,
                    )
                    .await?;
                    tx.commit().await.map_err(|error| {
                        anyhow::anyhow!("appraisal {id} commit acknowledgement unknown: {error}")
                    })?;
                    Ok::<_, anyhow::Error>(receipt)
                })
                .await
                .unwrap_or_else(|_| {
                    Err(anyhow::anyhow!(
                        "appraisal {id} commit acknowledgement timed out; outcome unknown"
                    ))
                });
                if let Err(error) = &result {
                    // Expected revocation/invalid drafts are caller outcomes,
                    // not a broken resource owner. SQL and uncertain commit
                    // failures remain visible after the writer is joined.
                    if failures.len() < 32
                        && (error.downcast_ref::<sea_orm::DbErr>().is_some()
                            || error.to_string().contains("acknowledgement"))
                    {
                        failures.push(format!("appraisal {id}: {error:#}"));
                    }
                }
                let _ = write.reply.send(result);
            }
            failures
        }));
        Ok(())
    }

    pub async fn submit(
        &self,
        input: AppraisalInput,
        drafts: Vec<AppraisalEvent>,
    ) -> anyhow::Result<AppraisalReceipt> {
        anyhow::ensure!(
            drafts.len() <= 64 && serde_json::to_vec(&drafts)?.len() <= 64 * 1024,
            "appraisal draft bounds exceeded"
        );
        let id = input.id();
        let (reply, ack) = oneshot::channel();
        {
            let gate = self.admission.lock().unwrap_or_else(|e| e.into_inner());
            anyhow::ensure!(!gate.closed, "appraisal writer closed");
            let sender = gate.sender.as_ref().ok_or_else(|| {
                anyhow::anyhow!("canonical appraisal persistence is not attached")
            })?;
            sender
                .try_send(Write {
                    input,
                    drafts,
                    reply,
                })
                .map_err(|error| {
                    anyhow::anyhow!(
                        "appraisal not admitted: {}",
                        match error {
                            mpsc::error::TrySendError::Full(_) => "queue full",
                            mpsc::error::TrySendError::Closed(_) => "writer stopped",
                        }
                    )
                })?;
        }
        tokio::time::timeout(Duration::from_secs(2), ack)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "appraisal {id} acknowledgement unknown; admitted mutation remains owned"
                )
            })?
            .map_err(|_| {
                anyhow::anyhow!(
                    "appraisal {id} writer lost acknowledgement; query original identity"
                )
            })?
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        {
            let mut gate = self.admission.lock().unwrap_or_else(|e| e.into_inner());
            gate.closed = true;
            gate.sender.take();
        }
        let mut task = self.task.lock().await;
        if let Some(handle) = task.as_mut() {
            let result = handle.await;
            task.take();
            let mut gate = self.admission.lock().unwrap_or_else(|e| e.into_inner());
            match result {
                Ok(failures) => gate.failures.extend(failures),
                Err(error) => gate
                    .failures
                    .push(format!("appraisal writer join failed: {error}")),
            }
            gate.failures.truncate(32);
        }
        let gate = self.admission.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            gate.failures.is_empty(),
            "appraisal persistence failures: {}",
            gate.failures.join("; ")
        );
        Ok(())
    }
}
