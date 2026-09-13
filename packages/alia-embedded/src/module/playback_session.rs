//! Local Ready owns the voice session. Input End releases only the input lease;
//! each bounded output observation survives until the backend reports its end.
use super::traits::AudioPlaybackBackend;
use alia_core::delivery::{ObservationEnd, PlaybackIdentity, PlaybackProgress, VoiceScope};
use anyhow::{Context, Result, ensure};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use uuid::Uuid;
mod submission;
pub use submission::{AudioSubmission, SubmissionWrite};

/// Other output owners follow the same locally activated voice epoch. Hooks
/// must only update admission/cancellation; no I/O or waits under this gate.
pub trait VoiceOutputFence: Send + Sync {
    fn activate_voice(&self, scope: &VoiceScope) -> Result<()>;
    fn fence_voice(&self, scope: &VoiceScope) -> Result<()>;
    fn close_voice(&self, scope: &VoiceScope) -> Result<()>;
}

#[derive(Clone, Debug)]
pub struct PlaybackLease {
    pub scope: VoiceScope,
    pub playback_id: String,
    pub playback_sequence: u64,
    pub sample_rate: u32,
    pub submission: Arc<AudioSubmission>,
    generation: u64,
}
impl PlaybackLease {
    pub fn identity(&self) -> PlaybackIdentity {
        self.submission.identity
    }
}
struct ActiveSession {
    scope: VoiceScope,
    highest_sequence: u64,
    generation: Option<u64>,
    input: Option<PlaybackIdentity>,
    observations: HashMap<PlaybackIdentity, PlaybackLease>,
}
pub struct PlaybackSession {
    backend: Arc<dyn AudioPlaybackBackend>,
    state: Mutex<Option<ActiveSession>>,
    tasks: tokio_util::task::TaskTracker,
    shutdown: tokio_util::sync::CancellationToken,
    task_gate: Mutex<()>,
    task_errors: Arc<Mutex<Vec<String>>>,
    output_fence: Option<Arc<dyn VoiceOutputFence>>,
}
impl PlaybackSession {
    pub fn new(backend: Arc<dyn AudioPlaybackBackend>) -> Self {
        Self {
            backend,
            state: Mutex::new(None),
            tasks: tokio_util::task::TaskTracker::new(),
            shutdown: tokio_util::sync::CancellationToken::new(),
            task_gate: Mutex::new(()),
            task_errors: Arc::default(),
            output_fence: None,
        }
    }

    pub fn with_output_fence(
        backend: Arc<dyn AudioPlaybackBackend>,
        output_fence: Arc<dyn VoiceOutputFence>,
    ) -> Self {
        Self {
            output_fence: Some(output_fence),
            ..Self::new(backend)
        }
    }

    /// Body admission is ordered with all local Ready/fence/close transitions.
    /// Binding authentication holds its read lock outside this lock; an output
    /// sink must never call back into PlaybackSession while holding its own gate.
    pub fn with_voice_admission<T>(
        &self,
        scope: &VoiceScope,
        admit: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let state = self.lock()?;
        ensure!(
            !self.shutdown.is_cancelled()
                && state.as_ref().is_some_and(|active| active.scope == *scope),
            "body voice scope is not locally current"
        );
        admit()
    }
    pub fn shutdown_token(&self) -> tokio_util::sync::CancellationToken {
        self.shutdown.clone()
    }
    pub fn spawn_push(
        &self,
        future: impl std::future::Future<Output = Result<()>> + Send + 'static,
    ) -> Result<()> {
        use futures::FutureExt;
        let _gate = self.task_gate.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(
            !self.shutdown.is_cancelled() && self.tasks.len() < 32,
            "playback runtime is stopped or full"
        );
        let errors = self.task_errors.clone();
        self.tasks.spawn(async move {
            let mut owned = Box::pin(future);
            match std::panic::AssertUnwindSafe(owned.as_mut())
                .catch_unwind()
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => errors
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(error.to_string()),
                Err(_) => {
                    errors
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push("playback Push worker panicked; cleanup is unproven".into());
                    std::future::pending::<()>().await;
                }
            }
            drop(owned);
        });
        Ok(())
    }
    fn lock(&self) -> Result<MutexGuard<'_, Option<ActiveSession>>> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("playback session lock poisoned"))
    }
    fn retire(&self, active: &mut ActiveSession, reason: &str) -> Result<()> {
        active.input = None;
        let result = active
            .generation
            .take()
            .map_or(Ok(false), |g| self.backend.stop_if_current(g));
        for (_, lease) in active.observations.drain() {
            lease.submission.terminate(if result.is_ok() {
                ObservationEnd::Cancelled {
                    reason: reason.into(),
                }
            } else {
                ObservationEnd::Unknown {
                    reason: "backend stop failed; output state is unknown".into(),
                }
            });
        }
        result.map(|_| ())
    }
    /// Only the local VoiceSession Ready hook may activate a session.
    pub fn activate(&self, scope: &VoiceScope) -> Result<()> {
        let _gate = self.task_gate.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(!self.shutdown.is_cancelled(), "playback runtime is stopped");
        ensure!(scope.is_valid(), "invalid playback voice scope");
        let mut state = self.lock()?;
        if let Some(active) = state.as_ref() {
            if same_session(&active.scope, scope) {
                ensure!(
                    scope.generation_epoch >= active.scope.generation_epoch,
                    "Ready epoch regressed"
                );
                drop(state);
                self.fence(scope, false)?;
                return Ok(());
            }
        }
        if let Some(mut old) = state.take() {
            let body = self
                .output_fence
                .as_ref()
                .map_or(Ok(()), |sink| sink.close_voice(&old.scope));
            let audio = self.retire(&mut old, "local voice session replaced");
            body.and(audio)?;
        }
        if let Some(sink) = &self.output_fence {
            sink.activate_voice(scope)?;
        }
        *state = Some(ActiveSession {
            scope: *scope,
            highest_sequence: 0,
            generation: None,
            input: None,
            observations: HashMap::new(),
        });
        Ok(())
    }
    pub fn close(&self, scope: &VoiceScope) -> Result<bool> {
        self.fence(scope, true)
    }
    pub fn fence(&self, minimum: &VoiceScope, close_session: bool) -> Result<bool> {
        ensure!(minimum.is_valid(), "invalid playback voice scope");
        let mut state = self.lock()?;
        let Some(active) = state.as_mut() else {
            return Ok(false);
        };
        if !same_session(&active.scope, minimum) {
            return Ok(false);
        }
        if close_session {
            let mut retired = state.take().expect("checked above");
            let body = self
                .output_fence
                .as_ref()
                .map_or(Ok(()), |sink| sink.close_voice(&retired.scope));
            let audio = self.retire(&mut retired, "matching voice session closed");
            body.and(audio)?;
            return Ok(true);
        }
        if minimum.generation_epoch <= active.scope.generation_epoch {
            return Ok(false);
        }
        // State is invalidated even when hardware stop fails.
        active.scope.generation_epoch = minimum.generation_epoch;
        let body = self
            .output_fence
            .as_ref()
            .map_or(Ok(()), |sink| sink.fence_voice(minimum));
        let audio = self.retire(active, "voice generation fence applied");
        body.and(audio)?;
        Ok(true)
    }
    pub fn begin(
        &self,
        scope: &VoiceScope,
        playback_id: &str,
        playback_sequence: u64,
        sample_rate: u32,
    ) -> Result<PlaybackLease> {
        let _gate = self.task_gate.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(!self.shutdown.is_cancelled(), "playback runtime is stopped");
        let identity = PlaybackIdentity {
            scope: *scope,
            playback_id: Uuid::parse_str(playback_id)?,
            playback_sequence,
        };
        ensure!(identity.is_valid(), "invalid playback identity");
        ensure!(
            (8000..=192000).contains(&sample_rate),
            "playback sample rate outside 8–192 kHz"
        );
        let mut state = self.lock()?;
        let active = state
            .as_mut()
            .context("no locally activated voice session")?;
        ensure!(
            same_session(&active.scope, scope),
            "playback session identity is stale"
        );
        ensure!(
            scope.generation_epoch >= active.scope.generation_epoch,
            "playback epoch is stale"
        );
        ensure!(
            playback_sequence > active.highest_sequence,
            "playback sequence was already used or is stale"
        );
        let newer = scope.generation_epoch > active.scope.generation_epoch;
        ensure!(
            newer || active.input.is_none(),
            "previous playback Push is still receiving media"
        );
        active.highest_sequence = playback_sequence;
        if newer {
            active.scope = *scope;
            let body = self
                .output_fence
                .as_ref()
                .map_or(Ok(()), |sink| sink.fence_voice(scope));
            let audio = self.retire(active, "new playback generation fence applied");
            body.and(audio)?;
        }
        active
            .observations
            .retain(|_, lease| lease.submission.is_live());
        ensure!(
            active.observations.len() < 32,
            "active playback observation capacity reached"
        );
        let generation = match active.generation {
            Some(g) if self.backend.is_playback_current(g) => g,
            _ => self.backend.begin_scoped_playback()?,
        };
        active.generation = Some(generation);
        let submission = self
            .backend
            .open_submission(identity, sample_rate, generation)?;
        let lease = PlaybackLease {
            scope: *scope,
            playback_id: playback_id.into(),
            playback_sequence,
            sample_rate,
            generation,
            submission,
        };
        active.input = Some(identity);
        active.observations.insert(identity, lease.clone());
        Ok(lease)
    }
    /// Never hold the session lock across media backpressure. Backend admission
    /// checks the expected generation and exact input identity atomically.
    pub fn enqueue(&self, lease: &PlaybackLease, pcm: &[i16]) -> Result<bool> {
        if !self
            .lock()?
            .as_ref()
            .is_some_and(|active| active.input == Some(lease.identity()))
        {
            return Ok(false);
        }
        self.backend
            .submit_audio(&lease.submission, lease.submission.accepted(), pcm)
    }
    pub fn end(&self, lease: &PlaybackLease) -> Result<bool> {
        if !self
            .lock()?
            .as_ref()
            .is_some_and(|active| active.input == Some(lease.identity()))
        {
            return Ok(false);
        }
        let ended = self
            .backend
            .end_submission(&lease.submission, lease.submission.accepted())?;
        if ended {
            if let Some(active) = self.lock()?.as_mut() {
                if active.input == Some(lease.identity()) {
                    active.input = None;
                }
            }
        }
        Ok(ended)
    }
    pub fn progress(&self, lease: &PlaybackLease, receipt_sequence: u64) -> PlaybackProgress {
        lease.submission.snapshot(receipt_sequence)
    }
    pub fn is_current(&self, lease: &PlaybackLease) -> Result<bool> {
        Ok(lease.submission.is_live()
            && self.backend.is_playback_current(lease.generation)
            && self
                .lock()?
                .as_ref()
                .is_some_and(|active| active.observations.contains_key(&lease.identity())))
    }
    pub fn cancel_identity(&self, identity: &PlaybackIdentity, reason: &str) -> Result<bool> {
        let lease = {
            let mut state = self.lock()?;
            let Some(active) = state.as_mut() else {
                return Ok(false);
            };
            let Some(lease) = active.observations.get(identity).cloned() else {
                return Ok(false);
            };
            lease
        };
        let result = self.backend.cancel_submission(&lease.submission, reason);
        if let Some(active) = self.lock()?.as_mut() {
            if active.input == Some(*identity) {
                active.input = None;
            }
        }
        match result {
            Ok(true) => lease.submission.terminate(ObservationEnd::Cancelled {
                reason: reason.into(),
            }),
            Ok(false) => {}
            Err(_) => lease.submission.terminate(ObservationEnd::Unknown {
                reason: "precise backend cancellation failed".into(),
            }),
        }
        result
    }
    /// A precise cancellation success includes the final settled software
    /// submission prefix. A native write still in flight is not a successful ACK.
    pub async fn cancel_observed(
        &self,
        identity: &PlaybackIdentity,
        reason: &str,
    ) -> Result<PlaybackProgress> {
        let lease = self
            .lock()?
            .as_ref()
            .and_then(|active| active.observations.get(identity).cloned())
            .context("playback identity is no longer observable")?;
        ensure!(
            self.cancel_identity(identity, reason)?,
            "precise playback cancellation was not applied"
        );
        tokio::time::timeout(std::time::Duration::from_millis(1500), async {
            while !lease.submission.output_quiescent() {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
            let progress = lease.submission.next_snapshot();
            ensure!(
                progress.observation_end.is_some(),
                "cancelled playback has no terminal evidence"
            );
            Ok(progress)
        })
        .await
        .context("native playback output did not settle before precise cancellation deadline")?
    }

    pub fn cancel(&self, lease: &PlaybackLease) -> Result<bool> {
        self.cancel_identity(&lease.identity(), "playback Push was retired")
    }
    pub fn is_active(&self) -> bool {
        self.state.lock().is_ok_and(|state| state.is_some())
    }
    pub fn active_scope(&self) -> Result<Option<VoiceScope>> {
        Ok(self.lock()?.as_ref().map(|active| active.scope))
    }
}
#[tonic::async_trait]
impl super::lifecycle::ModuleLifecycle for PlaybackSession {
    fn name(&self) -> &'static str {
        "playback_sessions"
    }
    async fn stop(&self) -> Result<()> {
        {
            let _gate = self.task_gate.lock().unwrap_or_else(|e| e.into_inner());
            self.shutdown.cancel();
            self.tasks.close();
        }
        let retired = if let Some(scope) = self.active_scope()? {
            self.close(&scope).map(|_| ())
        } else {
            Ok(())
        };
        // A pending native enqueue remains owned here until its control fence
        // actually releases it. The outer cleanup deadline retains this owner.
        self.tasks.wait().await;
        retired?;
        let errors = self.task_errors.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(
            errors.is_empty(),
            "playback workers failed: {}",
            errors.join("; ")
        );
        Ok(())
    }
}
fn same_session(left: &VoiceScope, right: &VoiceScope) -> bool {
    left.device_boot_id == right.device_boot_id && left.session_id == right.session_id
}
#[cfg(test)]
mod tests;
