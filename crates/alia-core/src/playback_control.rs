//! Reversible playback control. No wire, clock, permission lookup or native I/O.
//! Callers supply monotonic elapsed time and authenticated, fresh decisions.
use crate::delivery::VoiceScope;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_CONTROL_COMMANDS: usize = 32;
pub const MAX_HOLD_MS: u64 = 120_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HoldReason {
    AcousticCandidate,
    ExplicitWait,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResumeKind {
    FreshBackchannel,
    ExplicitContinue,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeAuthorization {
    pub kind: ResumeKind,
    pub detection_id: Uuid,
    pub through_sequence: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlAction {
    Pause {
        reason: HoldReason,
        through_sequence: u64,
        budget_ms: u64,
    },
    Resume {
        authorization: ResumeAuthorization,
    },
    /// Strengthen an existing acoustic hold without releasing it or extending
    /// its first absolute deadline. A new semantic decision must prove Wait.
    ConfirmExplicitWait {
        detection_id: Uuid,
        through_sequence: u64,
    },
}
/// Exact equality is the local immutable intent check. A future wire adapter
/// must authenticate this command; model labels alone cannot construct authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlCommand {
    pub scope: VoiceScope,
    pub hold_id: Uuid,
    pub command_id: Uuid,
    pub sequence: u64,
    pub action: ControlAction,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlPhase {
    Running,
    Pausing,
    Held,
    Retired,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetiredReason {
    Cancelled,
    Closed,
    Expired,
    BackendChanged,
    Capacity,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlRejection {
    Invalid,
    IdentityConflict,
    Sequence,
    AnotherHold,
    NotHeld,
    StaleEvidence,
    ExplicitContinueRequired,
    NativeWritePending,
    Retired,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandOutcome {
    Applied,
    Rejected(ControlRejection),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlSnapshot {
    pub scope: VoiceScope,
    pub phase: ControlPhase,
    pub hold_id: Option<Uuid>,
    pub reason: Option<HoldReason>,
    /// Accepted command watermark, including proved action rejections; this is
    /// not a counter of successful pause/resume actions.
    pub sequence: u64,
    pub remaining_ms: Option<u64>,
    pub retired: Option<RetiredReason>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlReceipt {
    pub command_id: Uuid,
    pub outcome: CommandOutcome,
    /// Current fact, distinct from the immutable outcome of this command.
    pub snapshot: ControlSnapshot,
}

#[derive(Debug)]
struct Hold {
    id: Uuid,
    reason: HoldReason,
    through: u64,
    expires_ms: u64,
}
#[derive(Debug)]
pub struct PlaybackControlState {
    scope: VoiceScope,
    hold: Option<Hold>,
    sequence: u64,
    retired: Option<RetiredReason>,
    capture: Option<u64>,
    speech: Option<u64>,
    commands: Vec<(ControlCommand, CommandOutcome)>,
}
impl PlaybackControlState {
    pub fn new(scope: VoiceScope) -> Result<Self, ControlRejection> {
        if !scope.is_valid() {
            return Err(ControlRejection::Invalid);
        }
        Ok(Self {
            scope,
            hold: None,
            sequence: 0,
            retired: None,
            capture: None,
            speech: None,
            commands: Vec::with_capacity(MAX_CONTROL_COMMANDS),
        })
    }
    /// The local capture owner supplies these facts before uploading the frame.
    /// A gap is not silently interpreted as silence and cannot authorize resume.
    pub fn observe_capture(&mut self, sequence: u64, speech: bool) -> Result<(), ControlRejection> {
        if self.retired.is_some() {
            return Err(ControlRejection::Retired);
        }
        if self
            .capture
            .is_some_and(|last| last.checked_add(1) != Some(sequence))
        {
            self.retire(RetiredReason::Closed);
            return Err(ControlRejection::StaleEvidence);
        }
        self.capture = Some(sequence);
        if speech {
            self.speech = Some(sequence);
        }
        Ok(())
    }
    /// Seed a newly created generation from the actual capture owner's retained
    /// contiguous watermarks. It cannot replace an existing clock or command.
    pub fn initialize_capture(
        &mut self,
        through_sequence: Option<u64>,
        latest_speech_sequence: Option<u64>,
    ) -> Result<(), ControlRejection> {
        if self.capture.is_some()
            || !self.commands.is_empty()
            || self.retired.is_some()
            || latest_speech_sequence
                .is_some_and(|speech| through_sequence.is_none_or(|last| speech > last))
        {
            return Err(ControlRejection::StaleEvidence);
        }
        self.capture = through_sequence;
        self.speech = latest_speech_sequence;
        Ok(())
    }
    pub fn retire(&mut self, reason: RetiredReason) {
        self.retired.get_or_insert(reason);
    }
    pub fn expire(&mut self, now_ms: u64) {
        if self.hold.as_ref().is_some_and(|h| now_ms >= h.expires_ms) {
            self.retire(RetiredReason::Expired);
        }
    }
    pub fn snapshot(&mut self, now_ms: u64, quiescent: bool) -> ControlSnapshot {
        self.expire(now_ms);
        ControlSnapshot {
            scope: self.scope,
            phase: if self.retired.is_some() {
                ControlPhase::Retired
            } else if self.hold.is_some() {
                if quiescent {
                    ControlPhase::Held
                } else {
                    ControlPhase::Pausing
                }
            } else {
                ControlPhase::Running
            },
            hold_id: self.hold.as_ref().map(|h| h.id),
            reason: self.hold.as_ref().map(|h| h.reason),
            sequence: self.sequence,
            remaining_ms: self
                .hold
                .as_ref()
                .map(|h| h.expires_ms.saturating_sub(now_ms)),
            retired: self.retired,
        }
    }
    /// Query an existing immutable outcome without accepting a command or
    /// changing its sequence, hold, deadline, or terminal state. A changed
    /// command with a known ID conflicts even after this owner is retired.
    pub fn recorded_outcome(&self, command: &ControlCommand) -> Option<CommandOutcome> {
        self.commands
            .iter()
            .find(|(original, _)| original.command_id == command.command_id)
            .map(|(original, outcome)| {
                if original == command {
                    *outcome
                } else {
                    CommandOutcome::Rejected(ControlRejection::IdentityConflict)
                }
            })
    }
    pub fn apply(
        &mut self,
        command: &ControlCommand,
        now_ms: u64,
        quiescent: bool,
    ) -> ControlReceipt {
        self.expire(now_ms);
        let outcome = if let Some(outcome) = self.recorded_outcome(command) {
            outcome
        } else if self.retired.is_some() {
            CommandOutcome::Rejected(ControlRejection::Retired)
        } else if self.commands.len() >= MAX_CONTROL_COMMANDS {
            self.retire(RetiredReason::Capacity);
            CommandOutcome::Rejected(ControlRejection::Retired)
        } else {
            let result = self.apply_new(command, now_ms, quiescent);
            let outcome = result.map_or_else(CommandOutcome::Rejected, |_| CommandOutcome::Applied);
            // Even a proved rejection owns its ID. A changed retry cannot become
            // accepted after new capture, a different hold or a completed write.
            self.commands.push((command.clone(), outcome));
            outcome
        };
        ControlReceipt {
            command_id: command.command_id,
            outcome,
            snapshot: self.snapshot(now_ms, quiescent),
        }
    }
    fn apply_new(
        &mut self,
        c: &ControlCommand,
        now: u64,
        quiescent: bool,
    ) -> Result<(), ControlRejection> {
        use ControlRejection::*;
        if c.scope != self.scope || c.hold_id.is_nil() || c.command_id.is_nil() {
            return Err(Invalid);
        }
        if self.sequence.checked_add(1) != Some(c.sequence) {
            return Err(Sequence);
        }
        // A well-formed command owns its sequence even when its action is
        // rejected. A later intent uses the next sequence, never replaces it.
        self.sequence = c.sequence;
        match c.action {
            ControlAction::Pause {
                reason,
                through_sequence,
                budget_ms,
            } => {
                if self.hold.is_some() {
                    return Err(AnotherHold);
                }
                if !(1..=MAX_HOLD_MS).contains(&budget_ms) {
                    return Err(Invalid);
                }
                if self
                    .commands
                    .iter()
                    .any(|(old, _)| old.hold_id == c.hold_id)
                {
                    return Err(AnotherHold);
                }
                if self.capture.is_none_or(|last| through_sequence > last) {
                    return Err(StaleEvidence);
                }
                let expires_ms = now.checked_add(budget_ms).ok_or(Invalid)?;
                self.hold = Some(Hold {
                    id: c.hold_id,
                    reason,
                    through: through_sequence,
                    expires_ms,
                });
            }
            ControlAction::Resume { authorization: a } => {
                let h = self.hold.as_ref().ok_or(NotHeld)?;
                if h.id != c.hold_id {
                    return Err(AnotherHold);
                }
                self.validate_decision(a.detection_id, a.through_sequence)?;
                if h.reason == HoldReason::ExplicitWait && a.kind != ResumeKind::ExplicitContinue {
                    return Err(ExplicitContinueRequired);
                }
                if !quiescent {
                    return Err(NativeWritePending);
                }
                self.hold = None;
            }
            ControlAction::ConfirmExplicitWait {
                detection_id,
                through_sequence,
            } => {
                let h = self.hold.as_ref().ok_or(NotHeld)?;
                if h.id != c.hold_id {
                    return Err(AnotherHold);
                }
                if h.reason != HoldReason::AcousticCandidate {
                    return Err(Invalid);
                }
                self.validate_decision(detection_id, through_sequence)?;
                let h = self.hold.as_mut().expect("hold was checked");
                h.reason = HoldReason::ExplicitWait;
                h.through = through_sequence;
            }
        }
        Ok(())
    }
    fn validate_decision(&self, detection_id: Uuid, through: u64) -> Result<(), ControlRejection> {
        let h = self.hold.as_ref().ok_or(ControlRejection::NotHeld)?;
        if detection_id.is_nil()
            || through < h.through
            || self.capture.is_none_or(|last| through > last)
            || self.speech.is_some_and(|last| through < last)
            || self.commands.iter().any(|(old, outcome)| {
                *outcome == CommandOutcome::Applied
                    && match old.action {
                        ControlAction::Resume { authorization } => {
                            authorization.detection_id == detection_id
                        }
                        ControlAction::ConfirmExplicitWait {
                            detection_id: old_id,
                            ..
                        } => old_id == detection_id,
                        ControlAction::Pause { .. } => false,
                    }
            })
        {
            return Err(ControlRejection::StaleEvidence);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> PlaybackControlState {
        let mut s = PlaybackControlState::new(VoiceScope {
            device_boot_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            generation_epoch: 3,
        })
        .unwrap();
        s.observe_capture(7, true).unwrap();
        s
    }
    fn pause(s: &PlaybackControlState, reason: HoldReason) -> ControlCommand {
        ControlCommand {
            scope: s.scope,
            command_id: Uuid::new_v4(),
            hold_id: Uuid::new_v4(),
            sequence: 1,
            action: ControlAction::Pause {
                reason,
                through_sequence: 7,
                budget_ms: 3_000,
            },
        }
    }
    fn resume(p: &ControlCommand, kind: ResumeKind) -> ControlCommand {
        ControlCommand {
            command_id: Uuid::new_v4(),
            sequence: 2,
            action: ControlAction::Resume {
                authorization: ResumeAuthorization {
                    kind,
                    detection_id: Uuid::new_v4(),
                    through_sequence: 7,
                },
            },
            ..p.clone()
        }
    }
    #[test]
    fn two_reasons_require_the_confirmed_distinct_authorizations() {
        for reason in [HoldReason::AcousticCandidate, HoldReason::ExplicitWait] {
            let mut s = state();
            let p = pause(&s, reason);
            assert_eq!(s.apply(&p, 0, true).outcome, CommandOutcome::Applied);
            let r = resume(&p, ResumeKind::FreshBackchannel);
            let receipt = s.apply(&r, 10, true);
            assert_eq!(
                receipt.outcome,
                if reason == HoldReason::ExplicitWait {
                    CommandOutcome::Rejected(ControlRejection::ExplicitContinueRequired)
                } else {
                    CommandOutcome::Applied
                }
            );
            if reason == HoldReason::ExplicitWait {
                let mut continued = resume(&p, ResumeKind::ExplicitContinue);
                continued.sequence = 3;
                assert_eq!(
                    s.apply(&continued, 11, true).snapshot.phase,
                    ControlPhase::Running
                );
            }
            assert_eq!(s.scope, p.scope);
        }
    }
    #[test]
    fn duplicate_pause_does_not_renew_and_retired_cannot_resume() {
        let mut s = state();
        let p = pause(&s, HoldReason::AcousticCandidate);
        assert_eq!(
            s.apply(&p, 100, false).snapshot.phase,
            ControlPhase::Pausing
        );
        assert_eq!(s.apply(&p, 3000, true).snapshot.remaining_ms, Some(100));
        assert_eq!(
            s.apply(&resume(&p, ResumeKind::FreshBackchannel), 3100, true)
                .snapshot
                .retired,
            Some(RetiredReason::Expired)
        );
        assert_eq!(s.apply(&p, 4000, true).outcome, CommandOutcome::Applied);
        assert_eq!(s.snapshot(4000, true).phase, ControlPhase::Retired);
    }
    #[test]
    fn new_speech_rejects_old_resume_and_rejection_cannot_be_rewritten() {
        let mut s = state();
        let p = pause(&s, HoldReason::AcousticCandidate);
        s.apply(&p, 0, true);
        s.observe_capture(8, true).unwrap();
        let mut r = resume(&p, ResumeKind::FreshBackchannel);
        assert_eq!(
            s.apply(&r, 1, true).outcome,
            CommandOutcome::Rejected(ControlRejection::StaleEvidence)
        );
        if let ControlAction::Resume { authorization } = &mut r.action {
            authorization.through_sequence = 8;
        }
        assert_eq!(
            s.apply(&r, 2, true).outcome,
            CommandOutcome::Rejected(ControlRejection::IdentityConflict)
        );
        r.command_id = Uuid::new_v4();
        r.sequence = 3;
        assert_eq!(s.apply(&r, 3, true).snapshot.phase, ControlPhase::Running);
    }
    #[test]
    fn write_inflight_capture_gap_capacity_and_cancel_do_not_release() {
        let mut s = state();
        let p = pause(&s, HoldReason::AcousticCandidate);
        s.apply(&p, 0, false);
        assert_eq!(
            s.apply(&resume(&p, ResumeKind::ExplicitContinue), 1, false)
                .outcome,
            CommandOutcome::Rejected(ControlRejection::NativeWritePending)
        );
        assert!(s.observe_capture(9, false).is_err());
        assert_eq!(s.snapshot(1, true).phase, ControlPhase::Retired);
        let mut s = state();
        for _ in 0..=MAX_CONTROL_COMMANDS {
            let mut c = pause(&s, HoldReason::ExplicitWait);
            c.sequence = 0;
            s.apply(&c, 1, true);
        }
        assert_eq!(s.snapshot(1, true).retired, Some(RetiredReason::Capacity));
        s.retire(RetiredReason::Cancelled);
        assert_eq!(s.snapshot(1, true).retired, Some(RetiredReason::Capacity));
    }
    #[test]
    fn explicit_wait_confirmation_strengthens_same_hold_without_refreshing_deadline() {
        let mut s = state();
        let p = pause(&s, HoldReason::AcousticCandidate);
        s.apply(&p, 0, true);
        let c = ControlCommand {
            command_id: Uuid::new_v4(),
            sequence: 2,
            action: ControlAction::ConfirmExplicitWait {
                detection_id: Uuid::new_v4(),
                through_sequence: 7,
            },
            ..p.clone()
        };
        let applied = s.apply(&c, 100, true);
        assert_eq!(applied.outcome, CommandOutcome::Applied);
        assert_eq!(applied.snapshot.reason, Some(HoldReason::ExplicitWait));
        assert_eq!(applied.snapshot.remaining_ms, Some(2900));
        assert_eq!(s.apply(&c, 2000, true).snapshot.remaining_ms, Some(1000));
        let mut r = resume(&p, ResumeKind::FreshBackchannel);
        r.sequence = 3;
        assert_eq!(
            s.apply(&r, 2100, true).outcome,
            CommandOutcome::Rejected(ControlRejection::ExplicitContinueRequired)
        );
        let mut r = resume(&p, ResumeKind::ExplicitContinue);
        r.sequence = 4;
        assert_eq!(
            s.apply(&r, 2200, true).snapshot.phase,
            ControlPhase::Running
        );
        let mut stale = state();
        stale.apply(&p, 0, true); // wrong scope rejected
        let p = pause(&stale, HoldReason::AcousticCandidate);
        stale.apply(&p, 0, true);
        stale.observe_capture(8, true).unwrap();
        let c = ControlCommand {
            command_id: Uuid::new_v4(),
            sequence: 2,
            action: ControlAction::ConfirmExplicitWait {
                detection_id: Uuid::new_v4(),
                through_sequence: 7,
            },
            ..p
        };
        assert_eq!(
            stale.apply(&c, 10, true).outcome,
            CommandOutcome::Rejected(ControlRejection::StaleEvidence)
        );
        assert_eq!(
            stale.snapshot(20, true).reason,
            Some(HoldReason::AcousticCandidate)
        );
        assert_eq!(
            stale.apply(&c, 3000, true).snapshot.retired,
            Some(RetiredReason::Expired)
        );
    }
    #[test]
    fn retained_capture_initialization_and_engineering_hold_limit_are_exact() {
        let mut s = state();
        assert!(s.initialize_capture(Some(100), Some(100)).is_err());
        let mut fresh = PlaybackControlState::new(s.scope).unwrap();
        assert!(fresh.initialize_capture(None, Some(1)).is_err());
        fresh.initialize_capture(Some(20), Some(19)).unwrap();
        let mut p = pause(&fresh, HoldReason::ExplicitWait);
        p.action = ControlAction::Pause {
            reason: HoldReason::ExplicitWait,
            through_sequence: 18,
            budget_ms: MAX_HOLD_MS,
        };
        assert_eq!(fresh.apply(&p, 0, true).outcome, CommandOutcome::Applied);
        let mut r = resume(&p, ResumeKind::ExplicitContinue);
        r.action = ControlAction::Resume {
            authorization: ResumeAuthorization {
                kind: ResumeKind::ExplicitContinue,
                detection_id: Uuid::new_v4(),
                through_sequence: 18,
            },
        };
        assert_eq!(
            fresh.apply(&r, 1, true).outcome,
            CommandOutcome::Rejected(ControlRejection::StaleEvidence)
        );
        assert_eq!(
            fresh.snapshot(MAX_HOLD_MS - 1, true).phase,
            ControlPhase::Held
        );
        assert_eq!(
            fresh.snapshot(MAX_HOLD_MS, true).retired,
            Some(RetiredReason::Expired)
        );
        assert_eq!(fresh.recorded_outcome(&p), Some(CommandOutcome::Applied));
        assert_eq!(
            fresh.apply(&p, MAX_HOLD_MS + 1, true).snapshot.remaining_ms,
            Some(0)
        );
        let mut over = pause(&s, HoldReason::ExplicitWait);
        if let ControlAction::Pause { budget_ms, .. } = &mut over.action {
            *budget_ms = MAX_HOLD_MS + 1;
        }
        assert_eq!(
            s.apply(&over, 0, true).outcome,
            CommandOutcome::Rejected(ControlRejection::Invalid)
        );
    }
}
