//! The original producer's reversible gate. No database transaction or native
//! I/O holds this lock. Expiry and cancellation can only close it permanently.
use alia_core::playback_control::{
    HoldReason, LocalCaptureHoldSnapshot, ResumeAuthorization, ResumeKind,
};
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

pub(super) const TEMPORARY_HOLD: Duration = Duration::from_secs(3);
pub(super) const DEFAULT_HOLD: Duration = Duration::from_secs(60);
pub(super) const OUTPUT_LIFETIME: Duration = Duration::from_secs(120);

#[derive(Clone, Debug)]
pub(super) struct Hold {
    pub id: Uuid,
    pub reason: HoldReason,
    pub through_sequence: u64,
    pub deadline: Instant,
    pub semantic_deadline: Option<Instant>,
}

#[derive(Debug)]
pub(super) struct Gate {
    pub hold: Option<Hold>,
    pub retired: bool,
    pub producer_finished: bool,
    pub media_owners: usize,
    pub last_speech: Option<u64>,
    pub deadline: Instant,
    pub budget: Duration,
}

impl Gate {
    pub fn new(now: Instant, budget: Duration) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !budget.is_zero() && budget <= OUTPUT_LIFETIME,
            "hold budget must be 1..120000 ms"
        );
        Ok(Self {
            hold: None,
            retired: false,
            producer_finished: false,
            media_owners: 0,
            last_speech: None,
            deadline: now + OUTPUT_LIFETIME,
            budget,
        })
    }
    pub fn deadline(&self) -> Instant {
        self.hold.as_ref().map_or(self.deadline, |hold| {
            hold.semantic_deadline
                .unwrap_or(hold.deadline)
                .min(hold.deadline)
                .min(self.deadline)
        })
    }
    pub fn is_alive(&self, now: Instant) -> bool {
        !self.retired
            && now < self.deadline()
            && !(self.producer_finished && self.media_owners == 0)
    }
    pub fn hold(
        &mut self,
        reason: HoldReason,
        through_sequence: u64,
        now: Instant,
    ) -> Option<Hold> {
        if !self.is_alive(now) || self.hold.is_some() {
            return None;
        }
        let hold = Hold {
            id: Uuid::new_v4(),
            reason,
            through_sequence,
            deadline: (now + self.budget).min(self.deadline),
            semantic_deadline: (reason == HoldReason::AcousticCandidate)
                .then_some(now + TEMPORARY_HOLD),
        };
        self.hold = Some(hold.clone());
        Some(hold)
    }
    /// The old Resume was applied, but a later device capture already closed
    /// its local gate. Replace only that completed formal hold, never the owner.
    pub fn handoff_after_applied_resume(
        &mut self,
        old_hold: Uuid,
        local: LocalCaptureHoldSnapshot,
        now: Instant,
    ) -> anyhow::Result<Option<Hold>> {
        anyhow::ensure!(
            self.is_alive(now) && local.remaining_ms > 0,
            "local capture hold expired"
        );
        let Some(old) = self.hold.as_ref() else {
            anyhow::bail!("original resume hold disappeared");
        };
        if old.id != old_hold {
            return Ok(None);
        }
        self.last_speech = Some(
            self.last_speech
                .map_or(local.through_sequence, |n| n.max(local.through_sequence)),
        );
        let deadline = old
            .deadline
            .min(self.deadline)
            .min(now + Duration::from_millis(local.remaining_ms));
        let hold = Hold {
            id: Uuid::new_v4(),
            reason: HoldReason::AcousticCandidate,
            through_sequence: local.through_sequence,
            deadline,
            semantic_deadline: Some((now + TEMPORARY_HOLD).min(deadline)),
        };
        self.hold = Some(hold.clone());
        Ok(Some(hold))
    }
    pub fn confirm(&mut self, through_sequence: u64, now: Instant) -> Option<Uuid> {
        if !self.is_alive(now) || self.last_speech.is_some_and(|n| n > through_sequence) {
            return None;
        }
        let hold = self.hold.as_mut()?;
        if hold.reason == HoldReason::ExplicitWait {
            return None;
        }
        hold.reason = HoldReason::ExplicitWait;
        hold.semantic_deadline = None;
        Some(hold.id)
    }
    pub fn can_resume(&self, hold_id: Uuid, evidence: ResumeAuthorization, now: Instant) -> bool {
        self.is_alive(now)
            && !evidence.detection_id.is_nil()
            && self
                .last_speech
                .is_none_or(|sequence| sequence <= evidence.through_sequence)
            && self.hold.as_ref().is_some_and(|hold| {
                hold.id == hold_id
                    && (hold.reason == HoldReason::AcousticCandidate
                        || evidence.kind == ResumeKind::ExplicitContinue)
            })
    }
    pub fn resume(&mut self, hold_id: Uuid, evidence: ResumeAuthorization, now: Instant) -> bool {
        if !self.can_resume(hold_id, evidence, now) {
            return false;
        }
        self.hold = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn authorization(kind: ResumeKind, through_sequence: u64) -> ResumeAuthorization {
        ResumeAuthorization {
            kind,
            through_sequence,
            detection_id: Uuid::new_v4(),
        }
    }
    #[test]
    fn confirmation_keeps_original_deadline_and_requires_explicit_continue() {
        let now = Instant::now();
        let mut gate = Gate::new(now, DEFAULT_HOLD).unwrap();
        gate.last_speech = Some(20);
        let hold = gate.hold(HoldReason::AcousticCandidate, 20, now).unwrap();
        assert_eq!(gate.deadline(), now + TEMPORARY_HOLD);
        assert_eq!(
            gate.confirm(20, now + Duration::from_secs(1)),
            Some(hold.id)
        );
        assert_eq!(gate.deadline(), now + DEFAULT_HOLD);
        assert!(
            gate.hold(HoldReason::ExplicitWait, 20, now + Duration::from_secs(10))
                .is_none()
        );
        assert!(gate.confirm(20, now + Duration::from_secs(10)).is_none());
        assert_eq!(gate.deadline(), hold.deadline);
        assert!(!gate.resume(
            hold.id,
            authorization(ResumeKind::FreshBackchannel, 20),
            now
        ));
        assert!(gate.resume(
            hold.id,
            authorization(ResumeKind::ExplicitContinue, 20),
            now
        ));
    }
    #[test]
    fn expiry_cancellation_and_new_speech_never_release_old_hold() {
        let now = Instant::now();
        let mut gate = Gate::new(now, DEFAULT_HOLD).unwrap();
        let hold = gate.hold(HoldReason::AcousticCandidate, 1, now).unwrap();
        gate.last_speech = Some(2);
        assert!(!gate.resume(hold.id, authorization(ResumeKind::FreshBackchannel, 1), now));
        assert!(!gate.resume(
            hold.id,
            authorization(ResumeKind::FreshBackchannel, 2),
            now + TEMPORARY_HOLD
        ));
        gate.retired = true;
        assert!(!gate.resume(hold.id, authorization(ResumeKind::ExplicitContinue, 2), now));
        assert!(!gate.is_alive(now));
    }
    #[test]
    fn original_output_lifetime_and_real_owner_counts_bound_retention() {
        let now = Instant::now();
        let mut gate = Gate::new(now, OUTPUT_LIFETIME).unwrap();
        gate.hold(HoldReason::ExplicitWait, 1, now + Duration::from_secs(119))
            .unwrap();
        assert_eq!(gate.deadline(), now + OUTPUT_LIFETIME);
        gate.producer_finished = true;
        gate.media_owners = 1;
        assert!(gate.is_alive(now));
        gate.media_owners = 0;
        assert!(!gate.is_alive(now));
    }

    #[test]
    fn applied_resume_handoff_keeps_owner_closed_and_never_refreshes_original_deadline() {
        let now = Instant::now();
        let mut gate = Gate::new(now, DEFAULT_HOLD).unwrap();
        let first = gate.hold(HoldReason::AcousticCandidate, 8, now).unwrap();
        let second = gate
            .handoff_after_applied_resume(
                first.id,
                LocalCaptureHoldSnapshot {
                    through_sequence: 9,
                    remaining_ms: 1000,
                },
                now + Duration::from_millis(50),
            )
            .unwrap()
            .unwrap();
        assert_ne!(second.id, first.id);
        assert_eq!(second.deadline, now + Duration::from_millis(1050));
        assert_eq!(gate.last_speech, Some(9));
        assert!(!gate.resume(
            first.id,
            authorization(ResumeKind::FreshBackchannel, 9),
            now
        ));
        assert!(!gate.resume(
            second.id,
            authorization(ResumeKind::FreshBackchannel, 8),
            now
        ));
        assert!(
            gate.handoff_after_applied_resume(
                first.id,
                LocalCaptureHoldSnapshot {
                    through_sequence: 10,
                    remaining_ms: 2000
                },
                now
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(gate.hold.as_ref().unwrap().deadline, second.deadline);
        assert!(gate.resume(
            second.id,
            authorization(ResumeKind::FreshBackchannel, 9),
            now
        ));
        assert!(gate.hold.is_none());
        assert_eq!(gate.deadline, now + OUTPUT_LIFETIME);
    }
}
