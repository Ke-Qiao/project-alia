//! Bounded, restartable shadow work over canonical SQL facts. No output adapter
//! or model is available to this service; a decision is never dispatch authority.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use sea_orm::DatabaseConnection;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::{TaskTracker, task_tracker::TaskTrackerToken};
use uuid::Uuid;

use super::repository::{CompanionRepository, RepoError, RepoResult, ShadowReport};
use super::types::{CompanionPolicy, OpportunitySnapshot, PolicySnapshot};
use crate::modules::interaction::initiative::VoiceInitiativeHandle;

const BATCH_LIMIT: u16 = 32;
const SCAN_INTERVAL: Duration = Duration::from_secs(5);

struct RequestPermit {
    _capacity: OwnedSemaphorePermit,
    _completion: TaskTrackerToken,
}

/// Opaque progress only: no owner, source IDs, private evidence or SQL errors.
#[derive(Clone, Debug, Default)]
pub struct ScanProgress {
    pub batches_started: u64,
    pub batches_completed: u64,
    pub sources_seen: u64,
    pub recorded: u64,
    pub rejected: u64,
    pub last_failure: Option<(&'static str, &'static str)>,
}

pub struct CompanionService {
    repository: CompanionRepository,
    voice: VoiceInitiativeHandle,
    instance_id: Uuid,
    cancel: CancellationToken,
    closed: AtomicBool,
    admission: std::sync::Mutex<()>,
    requests: Arc<Semaphore>,
    in_flight: TaskTracker,
    scan_progress: std::sync::Mutex<ScanProgress>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl CompanionService {
    pub fn new(
        db: DatabaseConnection,
        voice: VoiceInitiativeHandle,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            repository: CompanionRepository::new(db),
            voice,
            instance_id: Uuid::new_v4(),
            cancel: cancel.child_token(),
            closed: AtomicBool::new(false),
            admission: std::sync::Mutex::new(()),
            requests: Arc::new(Semaphore::new(8)),
            in_flight: TaskTracker::new(),
            scan_progress: std::sync::Mutex::new(ScanProgress::default()),
            worker: Mutex::new(None),
        }
    }

    fn admit(&self) -> RepoResult<RequestPermit> {
        let _boundary = self.admission.lock().expect("companion admission boundary");
        if self.closed.load(Ordering::Acquire) || self.cancel.is_cancelled() {
            return Err(RepoError::Unavailable("companion is stopping".into()));
        }
        let capacity = self
            .requests
            .clone()
            .try_acquire_owned()
            .map_err(|_| RepoError::Unavailable("companion request capacity reached".into()))?;
        Ok(RequestPermit {
            _capacity: capacity,
            _completion: self.in_flight.token(),
        })
    }

    /// Accepted public reads/writes still owned by their caller futures.
    pub fn active_requests(&self) -> usize {
        self.in_flight.len()
    }

    pub fn scan_progress(&self) -> ScanProgress {
        self.scan_progress
            .lock()
            .expect("companion progress")
            .clone()
    }

    fn scan_failure(&self, stage: &'static str, error: &RepoError) {
        let class = match error {
            RepoError::InvalidInput(_) => "invalid_input",
            RepoError::Conflict => "conflict",
            RepoError::Forbidden => "forbidden",
            RepoError::Erased => "erased",
            RepoError::Unavailable(_) => "unavailable",
        };
        self.scan_progress
            .lock()
            .expect("companion progress")
            .last_failure = Some((stage, class));
        tracing::warn!(stage, class, "companion shadow source unavailable");
    }

    pub async fn policy(&self, owner: Uuid) -> RepoResult<PolicySnapshot> {
        let _permit = self.admit()?;
        self.repository.policy(owner).await
    }

    pub async fn replace_policy(
        &self,
        owner: Uuid,
        expected: i64,
        policy: CompanionPolicy,
    ) -> RepoResult<PolicySnapshot> {
        let _permit = self.admit()?;
        self.repository
            .replace_policy(owner, expected, policy)
            .await
    }

    pub async fn evaluate(
        &self,
        owner: Uuid,
        operation: Uuid,
        sources: Vec<Uuid>,
    ) -> RepoResult<ShadowReport> {
        let _permit = self.admit()?;
        // A request contains references only. It cannot supply an idle/device
        // assertion. The worker separately obtains real runtime opportunities.
        self.repository.evaluate(owner, operation, sources).await
    }

    pub async fn replay(&self, owner: Uuid, operation: Uuid) -> RepoResult<ShadowReport> {
        let _permit = self.admit()?;
        self.repository.replay(owner, operation).await
    }

    pub async fn start(self: &Arc<Self>) -> anyhow::Result<()> {
        let mut worker = self.worker.lock().await;
        anyhow::ensure!(
            !self.closed.load(Ordering::Acquire) && !self.cancel.is_cancelled(),
            "companion already stopped"
        );
        if worker.is_some() {
            return Ok(());
        }
        let service = self.clone();
        *worker = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(SCAN_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = service.cancel.cancelled() => break,
                    _ = interval.tick() => {}
                }
                tokio::select! {
                    biased;
                    _ = service.cancel.cancelled() => break,
                    _ = service.scan_batch() => {}
                }
            }
        }));
        Ok(())
    }

    pub fn close(&self) {
        let _boundary = self.admission.lock().expect("companion admission boundary");
        self.closed.store(true, Ordering::Release);
        self.requests.close();
        self.in_flight.close();
        self.cancel.cancel();
    }

    pub async fn stop(&self) -> anyhow::Result<()> {
        self.close();
        let mut worker = self.worker.lock().await;
        let result = if let Some(task) = worker.as_mut() {
            // Keep the handle in its owner even if the caller's grace expires.
            let result = task.await;
            worker.take();
            result.map_err(anyhow::Error::from)
        } else {
            Ok(())
        };
        drop(worker);
        // A dropped stop waiter must not release the pool while an admitted
        // public request still owns a transaction. Tokens also drain on caller
        // cancellation; no detached replacement task retries an unknown commit.
        self.in_flight.wait().await;
        result
    }

    async fn scan_batch(&self) -> RepoResult<()> {
        self.scan_progress
            .lock()
            .expect("companion progress")
            .batches_started += 1;
        self.repository
            .prune_expired(BATCH_LIMIT)
            .await
            .inspect_err(|error| self.scan_failure("prune", error))?;
        let pending = self
            .repository
            .discovery_pending(BATCH_LIMIT)
            .await
            .inspect_err(|error| self.scan_failure("discovery", error))?;
        self.scan_progress
            .lock()
            .expect("companion progress")
            .sources_seen += pending.len() as u64;
        for observation in pending {
            if self.cancel.is_cancelled() {
                break;
            }
            let opportunity = self
                .opportunity(observation.owner_id, &observation.channel_id)
                .await;
            let result = self
                .repository
                .evaluate_with_opportunity(
                    observation.owner_id,
                    automatic_operation_id(observation.owner_id, observation.appraisal_id),
                    vec![observation.appraisal_id],
                    opportunity,
                )
                .await;
            match result {
                Ok(_) => {
                    self.scan_progress
                        .lock()
                        .expect("companion progress")
                        .recorded += 1
                }
                Err(
                    error @ (RepoError::Forbidden
                    | RepoError::Erased
                    | RepoError::Conflict
                    | RepoError::InvalidInput(_)),
                ) => {
                    self.scan_progress
                        .lock()
                        .expect("companion progress")
                        .rejected += 1;
                    self.scan_failure("evaluate", &error);
                }
                Err(error) => {
                    self.scan_failure("evaluate", &error);
                    return Err(error);
                }
            }
        }
        self.scan_progress
            .lock()
            .expect("companion progress")
            .batches_completed += 1;
        Ok(())
    }

    async fn opportunity(&self, owner: Uuid, channel: &str) -> Option<OpportunitySnapshot> {
        use super::types::{ActivityState, OpportunitySource, OutputCapability, OwnerAssurance};
        match self.voice.snapshot(owner, channel).await {
            Ok(value) => Some(OpportunitySnapshot {
                source: OpportunitySource::VerifiedLive {
                    server_instance_id: self.instance_id,
                    device_boot_id: value.scope.device_boot_id,
                    session_id: value.scope.session_id,
                    generation_epoch: value.scope.generation_epoch,
                },
                observed_at: value.observed_at,
                valid_until: value.expires_at,
                activity: ActivityState::Idle,
                owner_assurance: OwnerAssurance::BoundOwner,
                capability: OutputCapability::FollowUpVoice,
                capture_revision: Some(value.speech_revision),
                last_capture_at: Some(value.last_capture_at),
            }),
            // No observation occurred. The repository represents absence at its
            // own decision time, without comparing two unrelated clocks.
            Err(_) => None,
        }
    }
}

fn automatic_operation_id(owner: Uuid, appraisal: Uuid) -> Uuid {
    let hash = Sha256::digest(format!("alia.companion.shadow.v1:{owner}:{appraisal}"));
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}
