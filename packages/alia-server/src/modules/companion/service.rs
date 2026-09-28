//! Bounded work over canonical SQL facts. Shadow decisions never grant output;
//! the optional live runner requires a fresh durable permit and the voice actor.
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use sea_orm::DatabaseConnection;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tokio_util::task::{TaskTracker, task_tracker::TaskTrackerToken};
use uuid::Uuid;

use super::live::{LiveError, LiveRepository};
use super::repository::{CompanionRepository, RepoError, RepoResult, ShadowReport};
use super::runner::{LiveOutputs, LiveRunner, RunOutcome};
use super::types::{CompanionPolicy, OpportunitySnapshot, PolicySnapshot};
use crate::modules::interaction::initiative::VoiceInitiativeHandle;

const BATCH_LIMIT: u16 = 32;
const SCAN_INTERVAL: Duration = Duration::from_secs(5);
const LIVE_TASK_LIMIT: usize = 4;
const ATTEMPT_FENCE_LIMIT: usize = 1024;

#[cfg(test)]
mod tests;

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
    live_repository: Arc<LiveRepository>,
    live: Option<Arc<LiveRunner>>,
    voice: VoiceInitiativeHandle,
    instance_id: Uuid,
    cancel: CancellationToken,
    closed: AtomicBool,
    admission: std::sync::Mutex<()>,
    requests: Arc<Semaphore>,
    in_flight: TaskTracker,
    scan_progress: std::sync::Mutex<ScanProgress>,
    worker: Mutex<Option<JoinHandle<anyhow::Result<()>>>>,
}

impl CompanionService {
    pub fn new(
        db: DatabaseConnection,
        voice: VoiceInitiativeHandle,
        cancel: CancellationToken,
    ) -> Self {
        let instance_id = voice.instance_id();
        Self {
            repository: CompanionRepository::new(db.clone()),
            live_repository: Arc::new(LiveRepository::new(db, instance_id)),
            live: None,
            voice,
            instance_id,
            cancel: cancel.child_token(),
            closed: AtomicBool::new(false),
            admission: std::sync::Mutex::new(()),
            requests: Arc::new(Semaphore::new(8)),
            in_flight: TaskTracker::new(),
            scan_progress: std::sync::Mutex::new(ScanProgress::default()),
            worker: Mutex::new(None),
        }
    }

    /// Construction-time wiring only. Default policy remains shadow; Enabled
    /// also requires durable admission and the paired actor's local capture gate.
    pub(crate) fn with_live_outputs(mut self, outputs: LiveOutputs) -> Self {
        self.live = Some(Arc::new(LiveRunner::new(
            self.live_repository.clone(),
            self.voice.clone(),
            outputs,
            self.cancel.clone(),
        )));
        self
    }

    pub fn live_ready(&self) -> bool {
        self.live.is_some()
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
        if self.live_ready() {
            self.live_repository
                .replace_policy(owner, expected, policy)
                .await
                .map_err(live_error)
        } else {
            self.repository
                .replace_policy(owner, expected, policy)
                .await
        }
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
            let mut live_tasks = JoinSet::new();
            let mut active_channels = HashSet::new();
            // A failed/unknown frozen attempt cannot take another snapshot on
            // a later scan, even if its bounded SQL retirement was unavailable.
            // Entries expire only at the original candidate's fixed deadline.
            let mut attempted = HashMap::new();
            // Rotate past disconnected/busy devices without changing candidate
            // expiry or taking a second opportunity for a frozen attempt.
            let mut inspected = HashMap::new();
            let mut failure = None;
            let mut interval = tokio::time::interval(SCAN_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = service.cancel.cancelled() => break,
                    result = live_tasks.join_next(), if !live_tasks.is_empty() => {
                        match result.expect("nonempty live tasks") {
                            Ok((id, channel, outcome)) => {
                                active_channels.remove(&channel);
                                if matches!(outcome, RunOutcome::NoOpportunity) {
                                    attempted.remove(&id);
                                }
                                report_live_outcome(outcome);
                            }
                            Err(error) => {
                                service.close();
                                failure = Some(anyhow::Error::from(error));
                                break;
                            }
                        }
                        continue;
                    }
                    _ = interval.tick() => {}
                }
                tokio::select! {
                    biased;
                    _ = service.cancel.cancelled() => break,
                    result = tokio::time::timeout(SCAN_INTERVAL, service.scan_batch()) => {
                        if result.is_err() {
                            tracing::warn!("Companion shadow scan reached its batch deadline");
                        }
                    }
                }
                if let Some(runner) = &service.live {
                    attempted.retain(|_, deadline| *deadline > chrono::Utc::now());
                    inspected.retain(|_, deadline| *deadline > chrono::Utc::now());
                    let excluded: Vec<_> = attempted
                        .keys()
                        .chain(inspected.keys())
                        .copied()
                        .collect::<std::collections::HashSet<_>>()
                        .into_iter()
                        .collect();
                    let candidates = tokio::select! {
                        biased;
                        _ = service.cancel.cancelled() => break,
                        result = async {
                            service.live_repository.retire_batch(BATCH_LIMIT).await?;
                            service.live_repository.discover_pending_excluding(BATCH_LIMIT, &excluded).await
                        } => result,
                    };
                    match candidates {
                        Ok(candidates) => {
                            if candidates.is_empty() {
                                inspected.clear();
                            }
                            for pending in candidates {
                                if live_tasks.len() >= LIVE_TASK_LIMIT
                                    || attempted.len() + inspected.len() >= ATTEMPT_FENCE_LIMIT
                                {
                                    break;
                                }
                                if attempted.contains_key(&pending.candidate_id) {
                                    continue;
                                }
                                if !active_channels.insert(pending.channel_id.clone()) {
                                    inspected.insert(pending.candidate_id, pending.expires_at);
                                    continue;
                                }
                                let id = pending.candidate_id;
                                let channel = pending.channel_id.clone();
                                attempted.insert(id, pending.expires_at);
                                inspected.insert(id, pending.expires_at);
                                let runner = runner.clone();
                                live_tasks
                                    .spawn(async move { (id, channel, runner.run(pending).await) });
                            }
                        }
                        Err(_) => tracing::warn!(
                            "Companion live discovery unavailable; deferred to next scan"
                        ),
                    }
                }
            }
            service.cancel.cancel();
            // Child tokens are cancelled before draining. No JoinSet is dropped
            // with outstanding attempts while shared dependencies are released.
            while let Some(result) = live_tasks.join_next().await {
                match result {
                    Ok((_, _, outcome)) => report_live_outcome(outcome),
                    Err(error) if failure.is_none() => failure = Some(error.into()),
                    Err(_) => (),
                }
            }
            failure.map_or(Ok(()), Err)
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
            result
                .map_err(anyhow::Error::from)
                .and_then(std::convert::identity)
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

fn live_error(error: LiveError) -> RepoError {
    match error {
        LiveError::InvalidInput => RepoError::InvalidInput("invalid live Companion policy".into()),
        LiveError::Conflict => RepoError::Conflict,
        LiveError::Forbidden => RepoError::Forbidden,
        LiveError::Unavailable(_) | LiveError::CommitUnknown { .. } => {
            RepoError::Unavailable("live Companion completion unconfirmed".into())
        }
    }
}

fn report_live_outcome(outcome: RunOutcome) {
    match outcome {
        RunOutcome::Failed(stage) => tracing::warn!(
            stage,
            "original Companion attempt ended without confirmed output"
        ),
        RunOutcome::Observed(status) => {
            tracing::debug!(?status, "original Companion Delivery observation recorded")
        }
        RunOutcome::NoOpportunity | RunOutcome::Existing | RunOutcome::Retired => (),
    }
}
