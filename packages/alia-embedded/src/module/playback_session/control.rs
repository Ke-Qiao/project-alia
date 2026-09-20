//! One gate belongs to the original scope/native owner. Native callbacks only
//! touch its atomics; command history and elapsed-time checks stay off callback.
use alia_core::delivery::{SubmissionBackend, VoiceScope};
use alia_core::output_control::{
    BoundControlCommand, BoundControlReceipt, ControlMode, OutputBinding, OutputRegistration,
};
use alia_core::playback_control::*;
use anyhow::{Result, ensure};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::time::Instant;

/// Evidence for the implemented software gate, not physical presentation or a
/// successful native device pause. Retired alone does not prove write quiescence.
#[derive(Clone, Debug)]
pub struct PlaybackControlEvidence {
    pub control: ControlSnapshot,
    pub backend: SubmissionBackend,
    pub native_writes_quiescent: bool,
    pub progress: Vec<alia_core::delivery::PlaybackProgress>,
}

const CLOSED: usize = 1 << (usize::BITS - 1);
const HELD: usize = 1 << (usize::BITS - 2);
const COUNT: usize = HELD - 1;

#[derive(Debug)]
pub struct PlaybackControl {
    pub scope: VoiceScope,
    pub generation: u64,
    pub backend: SubmissionBackend,
    origin: Instant,
    gate: AtomicUsize,
    observation_sequence: AtomicU64,
    state: Mutex<PlaybackControlState>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputBlocked {
    Held,
    Retired,
}

impl PlaybackControl {
    pub(crate) fn new(
        scope: VoiceScope,
        generation: u64,
        backend: SubmissionBackend,
    ) -> Result<Arc<Self>> {
        let state = PlaybackControlState::new(scope)
            .map_err(|e| anyhow::anyhow!("invalid playback control: {e:?}"))?;
        Ok(Arc::new(Self {
            scope,
            generation,
            backend,
            origin: Instant::now(),
            gate: 0.into(),
            observation_sequence: 0.into(),
            state: Mutex::new(state),
        }))
    }
    fn now(&self) -> u64 {
        self.origin.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }
    fn synchronize(&self, phase: ControlPhase) {
        match phase {
            ControlPhase::Retired => {
                self.gate.fetch_or(CLOSED, Ordering::AcqRel);
            }
            ControlPhase::Pausing | ControlPhase::Held => {
                self.gate.fetch_or(HELD, Ordering::AcqRel);
            }
            ControlPhase::Running => {
                self.gate.fetch_and(!HELD, Ordering::AcqRel);
            }
        }
    }
    pub(crate) fn apply(&self, command: &ControlCommand) -> ControlReceipt {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = self.now();
        let mut receipt = state.apply(command, now, self.quiescent());
        self.synchronize(receipt.snapshot.phase);
        // A write may have entered between the first snapshot and closing the
        // gate. Only the post-gate count can prove a held submission boundary.
        receipt.snapshot = state.snapshot(now, self.quiescent());
        receipt
    }
    /// A receipt query never dispatches a command or reopens a gate. Reading
    /// the current clock may only seal an expired owner; history is unchanged.
    pub(crate) fn recorded_receipt(&self, command: &ControlCommand) -> Option<ControlReceipt> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let outcome = state.recorded_outcome(command)?;
        let snapshot = state.snapshot(self.now(), self.quiescent());
        if snapshot.phase == ControlPhase::Retired {
            self.gate.fetch_or(CLOSED, Ordering::AcqRel);
        }
        Some(ControlReceipt {
            command_id: command.command_id,
            outcome,
            snapshot,
        })
    }
    pub fn snapshot(&self) -> ControlSnapshot {
        self.with_snapshot(|snapshot| snapshot)
    }
    fn with_snapshot<T>(&self, read: impl FnOnce(ControlSnapshot) -> T) -> T {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let snapshot = state.snapshot(self.now(), self.quiescent());
        self.synchronize(snapshot.phase);
        read(snapshot)
    }
    pub(crate) fn initialize_capture(
        &self,
        through: Option<u64>,
        speech: Option<u64>,
    ) -> Result<()> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .initialize_capture(through, speech)
            .map_err(|e| anyhow::anyhow!("invalid initial capture barrier: {e:?}"))
    }
    pub(crate) fn observe_capture(&self, sequence: u64, speech: bool) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.expire(self.now());
        let result = state.observe_capture(sequence, speech);
        let snapshot = state.snapshot(self.now(), self.quiescent());
        self.synchronize(snapshot.phase);
        result.map_err(|e| anyhow::anyhow!("capture cannot authorize playback control: {e:?}"))
    }
    pub(crate) fn retire(&self, reason: RetiredReason) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.gate.fetch_or(CLOSED, Ordering::AcqRel);
        state.retire(reason);
    }
    pub fn is_retired(&self) -> bool {
        self.gate.load(Ordering::Acquire) & CLOSED != 0
    }
    pub fn is_held(&self) -> bool {
        self.gate.load(Ordering::Acquire) & (HELD | CLOSED) == HELD
    }
    pub fn quiescent(&self) -> bool {
        self.gate.load(Ordering::Acquire) & COUNT == 0
    }
    pub(crate) fn enter(self: &Arc<Self>) -> std::result::Result<ScopeWrite, OutputBlocked> {
        let mut gate = self.gate.load(Ordering::Acquire);
        loop {
            if gate & CLOSED != 0 {
                return Err(OutputBlocked::Retired);
            }
            if gate & HELD != 0 {
                return Err(OutputBlocked::Held);
            }
            if gate & COUNT == COUNT {
                return Err(OutputBlocked::Retired);
            }
            match self.gate.compare_exchange_weak(
                gate,
                gate + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(ScopeWrite(self.clone())),
                Err(current) => gate = current,
            }
        }
    }
}
#[derive(Debug)]
pub(crate) struct ScopeWrite(Arc<PlaybackControl>);
impl Drop for ScopeWrite {
    fn drop(&mut self) {
        self.0.gate.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OutputControlError {
    #[error("output registration or command is unknown")]
    Unknown,
    #[error("output binding, backend or immutable control intent conflicts")]
    Conflict,
}

impl super::PlaybackSession {
    /// A canonical response binds once to this exact local generation. The
    /// native owner is created here, before Start or any reversible command.
    pub fn register_output(&self, binding: &OutputBinding) -> Result<OutputRegistration> {
        ensure!(binding.is_valid(), "invalid output binding");
        let mut state = self.lock()?;
        let active = state
            .as_mut()
            .filter(|active| active.scope == binding.scope)
            .ok_or(OutputControlError::Unknown)?;
        ensure!(!self.shutdown.is_cancelled(), "playback runtime is stopped");
        if let Some(registered) = &active.registration {
            ensure!(registered.binding == *binding, OutputControlError::Conflict);
            return Ok(registered.clone());
        }
        ensure!(
            active.input.is_none() && active.observations.is_empty(),
            "output registration must precede media"
        );
        let capture = active
            .capture
            .as_ref()
            .map(|capture| capture.watermarks())
            .transpose()?;
        let uninitialized = active.control.is_none();
        let generation = self.control_generation(active)?;
        let control = self.backend.playback_control(&active.scope, generation)?;
        if uninitialized {
            if let Some((through, speech)) = capture {
                control.initialize_capture(through, speech)?;
            }
        }
        active.control = Some(control.clone());
        let registered = OutputRegistration {
            binding: *binding,
            backend: control.backend.clone(),
            maximum_hold_ms: active.maximum_hold_ms,
        };
        registered.validate()?;
        active.registration = Some(registered.clone());
        Ok(registered)
    }
    pub fn apply_bound_control(
        &self,
        command: &BoundControlCommand,
    ) -> Result<BoundControlReceipt> {
        self.bound_control(command, true)
    }
    pub fn observe_bound_control(
        &self,
        command: &BoundControlCommand,
    ) -> Result<BoundControlReceipt> {
        self.bound_control(command, false)
    }
    fn bound_control(
        &self,
        command: &BoundControlCommand,
        apply: bool,
    ) -> Result<BoundControlReceipt> {
        command.validate()?;
        let state = self.lock()?;
        ensure!(!self.shutdown.is_cancelled(), "playback runtime is stopped");
        let active = state
            .as_ref()
            .filter(|active| active.scope == command.binding.scope)
            .ok_or(OutputControlError::Unknown)?;
        let registered = active
            .registration
            .as_ref()
            .ok_or(OutputControlError::Unknown)?;
        ensure!(
            registered.binding == command.binding && registered.backend == command.expected_backend,
            OutputControlError::Conflict
        );
        if let ControlAction::Pause { budget_ms, .. } = command.command.action {
            ensure!(
                budget_ms <= registered.maximum_hold_ms,
                "hold budget exceeds negotiated capability"
            );
        }
        let control = active.control.as_ref().ok_or(OutputControlError::Unknown)?;
        if !self.backend.is_playback_current(control.generation) {
            control.retire(RetiredReason::BackendChanged);
        }
        let previous = control.recorded_receipt(&command.command);
        ensure!(
            !previous.as_ref().is_some_and(
                |r| r.outcome == CommandOutcome::Rejected(ControlRejection::IdentityConflict)
            ),
            OutputControlError::Conflict
        );
        let receipt = match previous {
            Some(receipt) => receipt,
            None if apply => self
                .backend
                .apply_playback_control(control, &command.command)?,
            None => return Err(OutputControlError::Unknown.into()),
        };
        let mut leases = active.observations.values().collect::<Vec<_>>();
        leases.sort_by_key(|lease| lease.playback_sequence);
        let result = control.with_snapshot(|snapshot| {
            // Snapshot and order assignment share the control mutex. A delayed
            // caller can never publish an old snapshot with a newer sequence.
            let observation_sequence = control
                .observation_sequence
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| anyhow::anyhow!("control observation sequence exhausted"))?
                + 1;
            Ok::<_, anyhow::Error>(BoundControlReceipt {
                observation_sequence,
                command: command.clone(),
                intent_digest: command.intent_digest(),
                receipt: ControlReceipt {
                    snapshot,
                    ..receipt
                },
                backend: control.backend.clone(),
                mode: ControlMode::SoftwareGate,
                native_writes_quiescent: control.quiescent(),
                progress: leases
                    .into_iter()
                    .map(|lease| lease.submission.next_snapshot())
                    .collect(),
            })
        })?;
        result.validate(command)?;
        Ok(result)
    }
    pub fn control_evidence(&self, scope: &VoiceScope) -> Result<PlaybackControlEvidence> {
        let state = self.lock()?;
        let active = state
            .as_ref()
            .filter(|a| a.scope == *scope)
            .ok_or_else(|| anyhow::anyhow!("control voice scope is stale"))?;
        let control = active
            .control
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("scope has no playback control owner"))?;
        if !self.backend.is_playback_current(control.generation) {
            control.retire(RetiredReason::BackendChanged);
        }
        let mut leases = active.observations.values().collect::<Vec<_>>();
        leases.sort_by_key(|lease| lease.playback_sequence);
        Ok(control.with_snapshot(|snapshot| PlaybackControlEvidence {
            control: snapshot,
            backend: control.backend.clone(),
            native_writes_quiescent: control.quiescent(),
            progress: leases
                .into_iter()
                .map(|lease| lease.submission.next_snapshot())
                .collect(),
        }))
    }
    /// Local production entry point. The caller authenticates binding and owns
    /// semantic authorization; no existing WAIT/network path invokes this yet.
    /// Exact retries query the retained original owner even after retirement.
    /// Closed/replaced scopes have no local registration and remain stale.
    pub fn apply_control(&self, command: &ControlCommand) -> Result<ControlReceipt> {
        let mut state = self.lock()?;
        let active = state
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("no active voice session"))?;
        ensure!(
            !self.shutdown.is_cancelled() && active.scope == command.scope,
            "control voice scope is stale"
        );
        if let Some(control) = &active.control {
            if !self.backend.is_playback_current(control.generation) {
                control.retire(RetiredReason::BackendChanged);
            }
            if let Some(receipt) = control.recorded_receipt(command) {
                return Ok(receipt);
            }
        }
        let generation = self.control_generation(active)?;
        let control = self.backend.playback_control(&active.scope, generation)?;
        active.control = Some(control.clone());
        self.backend.apply_playback_control(&control, command)
    }
    fn control_generation(&self, active: &mut super::ActiveSession) -> Result<u64> {
        ensure!(
            active.control.as_ref().is_none_or(|c| !c.is_retired()),
            "playback control scope was permanently retired"
        );
        let generation = match active.generation {
            Some(g) => {
                ensure!(
                    self.backend.is_playback_current(g),
                    "original native playback owner is gone"
                );
                g
            }
            None => self.backend.begin_scoped_playback()?,
        };
        active.generation = Some(generation);
        Ok(generation)
    }
    /// Feed the actual contiguous capture clock, including silence. This method
    /// does not infer speech or automatically pause/resume playback.
    pub fn observe_capture_for_control(
        &self,
        scope: &VoiceScope,
        sequence: u64,
        speech: bool,
    ) -> Result<()> {
        let mut state = self.lock()?;
        let active = state
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("no active voice session"))?;
        ensure!(
            !self.shutdown.is_cancelled() && active.scope == *scope,
            "capture control scope is stale"
        );
        let generation = self.control_generation(active)?;
        let control = self.backend.playback_control(scope, generation)?;
        active.control = Some(control.clone());
        control.observe_capture(sequence, speech)
    }
    pub fn observe_control(&self, scope: &VoiceScope) -> Result<ControlSnapshot> {
        let state = self.lock()?;
        let active = state
            .as_ref()
            .filter(|a| a.scope == *scope)
            .ok_or_else(|| anyhow::anyhow!("control voice scope is stale"))?;
        let control = active
            .control
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("scope has no playback control owner"))?;
        if !self.backend.is_playback_current(control.generation) {
            control.retire(RetiredReason::BackendChanged);
        }
        Ok(control.snapshot())
    }
    /// Dropping this waiter never drops the gate, native stream or expiration
    /// owner. Timeout is missing acknowledgement, not proof of Held/Rejected.
    pub async fn wait_held(
        &self,
        scope: &VoiceScope,
        hold_id: uuid::Uuid,
    ) -> Result<ControlSnapshot> {
        tokio::time::timeout(std::time::Duration::from_millis(1500), async {
            loop {
                let snapshot = self.observe_control(scope)?;
                ensure!(
                    snapshot.hold_id == Some(hold_id),
                    "another hold owns playback"
                );
                match snapshot.phase {
                    ControlPhase::Held => return Ok(snapshot),
                    ControlPhase::Pausing => {
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await
                    }
                    _ => anyhow::bail!("playback is not held: {:?}", snapshot.phase),
                }
            }
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!("native playback did not settle before hold acknowledgement deadline")
        })?
    }
}
