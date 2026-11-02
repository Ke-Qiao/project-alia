//! One owned keyset scan repairs missing projections. Canonical rows are the
//! durable work source; retries prepare new work rather than re-signing old work.
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::embedding::{
    DB_BUDGET, MemoryEmbeddingWork, REBUILD_BUDGET, WorkSeed, WriteResult, sql,
};
use super::*;
use crate::modules::memory::context::ContextPolicyStore;
use crate::modules::platform::background::OwnedBackground;

const BATCH_SIZE: i64 = 16;
const PROJECTION_CAPACITY: u32 = 2;
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

pub(super) struct ProjectionRuntime {
    background: OwnedBackground,
    cancel: CancellationToken,
    capacity: Arc<Semaphore>,
}

impl Default for ProjectionRuntime {
    fn default() -> Self {
        Self {
            background: OwnedBackground::default(),
            cancel: CancellationToken::new(),
            capacity: Arc::new(Semaphore::new(PROJECTION_CAPACITY as usize)),
        }
    }
}

impl ProjectionRuntime {
    pub(super) async fn execute(
        &self,
        db: &DatabaseConnection,
        adapter: &dyn EmbeddingAdapter,
        diagnostics: &RwLock<RuntimeDiagnostics>,
        work: &MemoryEmbeddingWork,
        request_budget: Duration,
    ) {
        let result = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return,
            result = async {
                let _permit = tokio::time::timeout(DB_BUDGET, self.capacity.clone().acquire_owned())
                    .await.map_err(|_| anyhow::anyhow!("Memory projection capacity is busy"))??;
                embedding::project(db, adapter, work, request_budget).await
            } => result,
        };
        let mut diagnostics = diagnostics.write().await;
        match result {
            Ok(WriteResult::Stored | WriteResult::AlreadyPresent) => {
                diagnostics.embedding = "available".into();
                diagnostics.embedding_probe = "profiled_projection_confirmed".into();
                diagnostics.retrieval = "profiled_vector_graph_policy".into();
                diagnostics.last_error = None;
            }
            Ok(WriteResult::Stale) => {
                // The old work says nothing about the availability of a newer
                // profile, so it must not overwrite current health with success.
            }
            Err(error) => {
                tracing::debug!(memory_id=%work.seed.id,error=?error,"Memory projection was not confirmed");
                diagnostics.embedding = "write_failed".into();
                diagnostics.retrieval = "keyword_overlap".into();
                diagnostics.last_error = Some(error.to_string());
            }
        }
    }

    pub(super) async fn start(
        self: &Arc<Self>,
        db: DatabaseConnection,
        adapter: Arc<RwLock<Option<Arc<dyn EmbeddingAdapter>>>>,
        diagnostics: Arc<RwLock<RuntimeDiagnostics>>,
        parent: &CancellationToken,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(!self.cancel.is_cancelled(), "Memory projection is stopping");
        let mut startup = self.background.begin_start(parent).await?;
        let token = startup.token();
        let cancel = self.cancel.clone();
        startup.spawn_owned("Memory projection cancellation", async move {
            token.cancelled().await;
            cancel.cancel();
            Ok(())
        });
        let runtime = self.clone();
        startup.spawn_loop("Memory embedding rebuild", async move {
            let mut cursor = None;
            loop {
                let result = scan(&db, cursor).await;
                // A successful scan confirms DB access, not a failed projection write.
                diagnostics.write().await.rebuild_scan_error =
                    result.as_ref().err().map(ToString::to_string);
                match result {
                    Ok(seeds) if !seeds.is_empty() => {
                        for seed in seeds {
                            // Advance even when inference or validation fails.
                            // A permanently bad first row cannot starve later rows.
                            cursor = Some((seed.owner, seed.id));
                            let prepared = tokio::time::timeout(DB_BUDGET, async {
                                let tx = db.begin().await?;
                                tx.execute(sql("SET LOCAL statement_timeout='2s'", vec![])).await?;
                                let work = embedding::prepare(&tx, seed).await?;
                                tx.commit().await?;
                                Ok::<_, anyhow::Error>(work)
                            }).await;
                            match prepared {
                                Ok(Ok(Some(work))) => {
                                    if let Some(adapter) = adapter.read().await.clone() {
                                        runtime.execute(&db, adapter.as_ref(), &diagnostics, &work, REBUILD_BUDGET).await;
                                    }
                                }
                                Ok(Ok(None)) => {}
                                error => tracing::debug!(error=?error,"Memory rebuild preparation was not confirmed"),
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Ok(_) => {
                        cursor = None;
                        tokio::time::sleep(SWEEP_INTERVAL).await;
                    }
                    Err(error) => {
                        tracing::warn!(error=?error,"Memory rebuild scan failed");
                        tokio::time::sleep(SWEEP_INTERVAL).await;
                    }
                }
            }
        });
        startup.commit();
        Ok(())
    }

    pub(super) async fn stop(&self) -> anyhow::Result<()> {
        self.cancel.cancel();
        self.background.stop().await?;
        // Inline projections also observe cancel. Keep their permits owned
        // until their SQL/network futures have returned or been dropped.
        let _joined = self.capacity.acquire_many(PROJECTION_CAPACITY).await?;
        Ok(())
    }
}

async fn scan(
    db: &DatabaseConnection,
    cursor: Option<(Uuid, Uuid)>,
) -> anyhow::Result<Vec<WorkSeed>> {
    tokio::time::timeout(DB_BUDGET, async {
        let policy = ContextPolicyStore::new(db.clone()).current().await?;
        if !policy.policy.reads_memory() { return Ok(vec![]); }
        let Some(profile) = policy.policy.active_embedding_profile else { return Ok(vec![]); };
        let rows = db.query_all(sql(
            "SELECT m.id,m.user_id,m.revision,p.privacy_epoch FROM memory_items m JOIN users u ON u.id=m.user_id AND u.status='active' JOIN memory_privacy_state p ON p.user_id=m.user_id WHERE m.tenant_id=$1 AND m.status='active' AND NOT m.sensitive AND m.visibility<>'private_runtime' AND (m.expires_at IS NULL OR m.expires_at>clock_timestamp()) AND ($3::uuid IS NULL OR (m.user_id,m.id)>($3,$4::uuid)) AND NOT EXISTS(SELECT 1 FROM memory_embeddings e WHERE e.memory_id=m.id AND e.profile_id=$2 AND e.source_revision=m.revision) ORDER BY m.user_id,m.id LIMIT $5",
            vec![DEFAULT_TENANT_ID.into(),profile.into(),cursor.map(|c|c.0).into(),cursor.map(|c|c.1).into(),BATCH_SIZE.into()],
        )).await?;
        rows.into_iter().map(|row| Ok(WorkSeed {
            owner: row.try_get("", "user_id")?, id: row.try_get("", "id")?,
            revision: row.try_get("", "revision")?, epoch: row.try_get("", "privacy_epoch")?,
            policy_version: policy.version,
        })).collect()
    }).await.map_err(|_| anyhow::anyhow!("Memory rebuild scan timed out"))?
}
