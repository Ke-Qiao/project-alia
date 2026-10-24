//! Local Ready owns the voice session. Input End releases only the input lease;
//! each bounded output observation survives until the backend reports its end.
use super::traits::AudioPlaybackBackend;
use alia_core::delivery::{ObservationEnd, PlaybackIdentity, PlaybackProgress, VoiceScope};
use anyhow::{Context, Result, ensure};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use uuid::Uuid;
mod capture;
mod control;
mod cue;
mod retirement;
mod submission;
pub use capture::CaptureControlBarrier;
pub use control::{OutputBlocked, OutputControlError, PlaybackControl, PlaybackControlEvidence};
pub use submission::{AudioSubmission, SubmissionWrite};

fn record_task_error(errors: &Mutex<Vec<String>>, error: String) {
    tracing::warn!(%error, "playback owner failed");
    let mut errors = errors.lock().unwrap_or_else(|e| e.into_inner());
    if errors.len() < 64 && !errors.contains(&error) {
        errors.push(error);
    }
}

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
    local_cue: bool,
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
    control: Option<Arc<PlaybackControl>>,
    registration: Option<alia_core::output_control::OutputRegistration>,
    capture: Option<CaptureControlBarrier>,
    initiative_event: Option<(alia_core::output_control::InitiativeCapture, Uuid)>,
    maximum_hold_ms: u64,
    retired: bool,
}
pub struct PlaybackSession {
    backend: Arc<dyn AudioPlaybackBackend>,
    state: Mutex<Option<ActiveSession>>,
    tasks: tokio_util::task::TaskTracker,
    shutdown: tokio_util::sync::CancellationToken,
    task_gate: Mutex<()>,
    task_errors: Arc<Mutex<Vec<String>>>,
    output_fence: Option<Arc<dyn VoiceOutputFence>>,
    retired: Mutex<std::collections::VecDeque<retirement::RetiredOutput>>,
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
            retired: Mutex::default(),
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

    /// Motion admission is ordered with all local Ready/fence/close transitions.
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
                && state
                    .as_ref()
                    .is_some_and(|active| active.scope == *scope && !active.retired),
            "motion voice scope is not locally current"
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
                Ok(Err(error)) => record_task_error(&errors, error.to_string()),
                Err(_) => {
                    record_task_error(
                        &errors,
                        "playback Push worker panicked; cleanup is unproven".into(),
                    );
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
        if active.retired {
            return Ok(());
        }
        active.retired = true;
        active.input = None;
        active.initiative_event = None;
        if let Some(control) = active.control.as_ref() {
            control.retire(alia_core::playback_control::RetiredReason::Cancelled);
        }
        let result = active
            .generation
            .map_or(Ok(false), |g| self.backend.stop_if_current(g));
        for lease in active.observations.values() {
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
        // Every live scope reserves one slot before native admission. Retired
        // owners retain their original gates and observations until settled.
        let mut retired = self.retired.lock().unwrap_or_else(|e| e.into_inner());
        debug_assert!(retired.len() < retirement::RETIRED_CAPACITY);
        retired.push_back(retirement::RetiredOutput {
            capture: active.capture.clone(),
            scope: active.scope,
            registration: active.registration.take(),
            control: active.control.take(),
            generation: active.generation.take(),
            observations: std::mem::take(&mut active.observations),
            stop_error: result.as_ref().err().map(|error| error.to_string()),
        });
        drop(retired);
        let result = result.map(|_| ());
        self.remember_retirement_error(active.scope, "native", &result);
        result
    }
    /// Only the local VoiceSession Ready hook may activate a session.
    pub fn activate(&self, scope: &VoiceScope) -> Result<()> {
        self.activate_local(scope, None, alia_core::playback_control::MAX_HOLD_MS)
    }
    pub fn activate_with_capture(
        &self,
        scope: &VoiceScope,
        capture: CaptureControlBarrier,
        maximum_hold_ms: u64,
    ) -> Result<()> {
        self.activate_local(scope, Some(capture), maximum_hold_ms)
    }
    fn activate_local(
        &self,
        scope: &VoiceScope,
        capture: Option<CaptureControlBarrier>,
        maximum_hold_ms: u64,
    ) -> Result<()> {
        let _gate = self.task_gate.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(!self.shutdown.is_cancelled(), "playback runtime is stopped");
        ensure!(scope.is_valid(), "invalid playback voice scope");
        ensure!(
            (1..=alia_core::playback_control::MAX_HOLD_MS).contains(&maximum_hold_ms),
            "invalid negotiated hold limit"
        );
        let mut state = self.lock()?;
        ensure!(
            !self
                .retired
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .any(|owner| owner.scope == *scope),
            "Ready cannot reactivate a retained retired output epoch"
        );
        if let Some(capture) = &capture {
            capture.watermarks()?;
        }
        if let Some(active) = state.as_mut() {
            if same_session(&active.scope, scope) {
                ensure!(
                    scope.generation_epoch >= active.scope.generation_epoch,
                    "Ready epoch regressed"
                );
                ensure!(
                    match (&active.capture, &capture) {
                        (None, None) => true,
                        (Some(old), Some(new)) => old.same_owner(new),
                        _ => false,
                    } && active.maximum_hold_ms == maximum_hold_ms,
                    "Ready cannot replace the capture owner or capabilities"
                );
                if scope.generation_epoch > active.scope.generation_epoch {
                    let motion = self
                        .output_fence
                        .as_ref()
                        .map_or(Ok(()), |sink| sink.fence_voice(scope));
                    let audio = self.retire(active, "local voice generation advanced");
                    self.remember_retirement_error(active.scope, "motion", &motion);
                    self.reserve_retirement_slot()?;
                    active.scope = *scope;
                    active.retired = false;
                    motion.and(audio)?;
                }
                ensure!(
                    !active.retired,
                    "Ready cannot reopen a retired output epoch"
                );
                return Ok(());
            }
        }
        if let Some(mut old) = state.take() {
            if let Some(capture) = &old.capture {
                capture.mark_retired();
            }
            let motion = self
                .output_fence
                .as_ref()
                .map_or(Ok(()), |sink| sink.close_voice(&old.scope));
            let audio = self.retire(&mut old, "local voice session replaced");
            self.remember_retirement_error(old.scope, "motion", &motion);
            motion.and(audio)?;
        }
        self.reserve_retirement_slot()?;
        if let Some(sink) = &self.output_fence {
            sink.activate_voice(scope)?;
        }
        *state = Some(ActiveSession {
            scope: *scope,
            highest_sequence: 0,
            generation: None,
            input: None,
            observations: HashMap::new(),
            control: None,
            registration: None,
            initiative_event: None,
            capture,
            maximum_hold_ms,
            retired: false,
        });
        Ok(())
    }
    /// Binding revocation retires the whole current owner under the same lock
    /// used by Ready, registration, capture, and control admission.
    pub fn revoke_all(&self) -> Result<()> {
        let mut state = self.lock()?;
        if let Some(mut retired) = state.take() {
            if let Some(capture) = &retired.capture {
                capture.mark_retired();
            }
            let motion = self
                .output_fence
                .as_ref()
                .map_or(Ok(()), |sink| sink.close_voice(&retired.scope));
            let audio = self.retire(&mut retired, "device binding revoked");
            self.remember_retirement_error(retired.scope, "motion", &motion);
            motion.and(audio)?;
        }
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
            if let Some(capture) = &retired.capture {
                capture.mark_retired();
            }
            let motion = self
                .output_fence
                .as_ref()
                .map_or(Ok(()), |sink| sink.close_voice(&retired.scope));
            let audio = self.retire(&mut retired, "matching voice session closed");
            self.remember_retirement_error(retired.scope, "motion", &motion);
            motion.and(audio)?;
            return Ok(true);
        }
        if minimum.generation_epoch <= active.scope.generation_epoch {
            return Ok(false);
        }
        // State is invalidated even when hardware stop fails.
        let motion = self
            .output_fence
            .as_ref()
            .map_or(Ok(()), |sink| sink.fence_voice(minimum));
        let audio = self.retire(active, "voice generation fence applied");
        self.remember_retirement_error(active.scope, "motion", &motion);
        self.reserve_retirement_slot()?;
        active.scope.generation_epoch = minimum.generation_epoch;
        active.retired = false;
        motion.and(audio)?;
        Ok(true)
    }
    /// The independently authenticated Register is safe even when its RPC wins
    /// the race with this voice event. Neither ordering may replace its gate.
    pub fn admit_initiative_event(
        &self,
        scope: &VoiceScope,
        capture: alia_core::output_control::InitiativeCapture,
        context_id: Uuid,
    ) -> Result<()> {
        ensure!(
            capture.is_valid() && !context_id.is_nil(),
            "invalid initiative event identity"
        );
        self.fence(scope, false)?;
        let mut state = self.lock()?;
        let active = state
            .as_mut()
            .filter(|active| active.scope == *scope && !active.retired)
            .ok_or_else(|| anyhow::anyhow!("initiative event scope is stale"))?;
        if let Some(previous) = active.initiative_event {
            ensure!(
                previous == (capture, context_id),
                "initiative event changed original binding"
            );
        }
        if let Some(registration) = &active.registration {
            ensure!(
                registration.binding.initiative_capture == Some(capture)
                    && registration.binding.context_id == context_id,
                "initiative event differs from registered output"
            );
        }
        active.initiative_event = Some((capture, context_id));
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn begin(
        &self,
        scope: &VoiceScope,
        playback_id: &str,
        playback_sequence: u64,
        sample_rate: u32,
    ) -> Result<PlaybackLease> {
        self.begin_local(scope, playback_id, playback_sequence, sample_rate, None)
    }
    pub fn begin_bound(
        &self,
        binding: &alia_core::output_control::OutputBinding,
        playback_id: &str,
        playback_sequence: u64,
        sample_rate: u32,
    ) -> Result<PlaybackLease> {
        self.begin_local(
            &binding.scope,
            playback_id,
            playback_sequence,
            sample_rate,
            Some(binding),
        )
    }
    fn begin_local(
        &self,
        scope: &VoiceScope,
        playback_id: &str,
        playback_sequence: u64,
        sample_rate: u32,
        binding: Option<&alia_core::output_control::OutputBinding>,
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
            !active.retired || scope.generation_epoch > active.scope.generation_epoch,
            "playback output epoch is permanently retired"
        );
        match binding {
            Some(binding) => ensure!(
                active.scope == *scope
                    && active
                        .registration
                        .as_ref()
                        .is_some_and(|registered| registered.binding == *binding),
                "Start does not match the pre-registered output"
            ),
            None => ensure!(
                active.registration.is_none(),
                "registered output requires its exact binding"
            ),
        }
        if binding.is_some_and(|binding| binding.initiative_capture.is_some()) {
            let (through, speech) = active
                .capture
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("initiative capture owner absent"))?
                .watermarks()?;
            active
                .control
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("initiative control owner absent"))?
                .validate_initiative_start(through, speech)?;
        }
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
            let motion = self
                .output_fence
                .as_ref()
                .map_or(Ok(()), |sink| sink.fence_voice(scope));
            let audio = self.retire(active, "new playback generation fence applied");
            self.remember_retirement_error(active.scope, "motion", &motion);
            self.reserve_retirement_slot()?;
            active.scope = *scope;
            active.retired = false;
            motion.and(audio)?;
        }
        ensure!(
            active.control.as_ref().is_none_or(|c| !c.is_retired()),
            "playback control permanently retired this generation"
        );
        active
            .observations
            .retain(|_, lease| lease.submission.is_live());
        ensure!(
            active.observations.len() < 32,
            "active playback observation capacity reached"
        );
        let generation = match active.generation {
            Some(g) if self.backend.is_playback_current(g) => g,
            _ if binding.is_none() => self.backend.begin_scoped_playback()?,
            _ => anyhow::bail!("registered native playback owner is gone"),
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
            local_cue: false,
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
            let state = self.lock()?;
            match state
                .as_ref()
                .and_then(|active| active.observations.get(identity).cloned())
            {
                Some(lease) => lease,
                None => return Ok(self.retired_lease(identity).is_some()),
            }
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
        ensure!(
            self.cancel_identity(identity, reason)?,
            "precise playback cancellation was not applied"
        );
        self.observe_cancelled(identity).await
    }
    /// Observe the original identity after authenticated synchronous cancellation.
    pub async fn observe_cancelled(&self, identity: &PlaybackIdentity) -> Result<PlaybackProgress> {
        let lease = self
            .lock()?
            .as_ref()
            .and_then(|active| active.observations.get(identity).cloned())
            .or_else(|| self.retired_lease(identity))
            .context("playback identity is no longer observable")?;
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
        self.wait_retired_writes().await;
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
