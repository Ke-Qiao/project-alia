//! Deterministic interpretation of model evidence; the caller owns audio and effects.

use crate::voice::{DetectionId, TurnObservation, TurnState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnDecision {
    /// Evidence was superseded or consumed already. It must have no side effects.
    Stale,
    /// Retain this utterance and wait for more near-end speech.
    ContinueListening,
    /// An acknowledgement does not replace the answer being spoken.
    Backchannel,
    /// Hold output until an explicit subsequent user turn permits progress.
    Wait,
    Respond {
        generation_epoch: u64,
        transcript: String,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("generation epoch exhausted; open a new session")]
pub struct EpochExhausted;

/// One authoritative controller per live voice session. Epochs are scoped to
/// that session, never compared across sessions or device boots.
#[derive(Debug)]
pub struct TurnController {
    epoch: u64,
    pending: Option<(DetectionId, u64)>,
}

impl Default for TurnController {
    fn default() -> Self {
        Self {
            epoch: 1,
            pending: None,
        }
    }
}

impl TurnController {
    pub fn generation_epoch(&self) -> u64 {
        self.epoch
    }

    /// revision counts near-end speech changes, not silence received during inference.
    pub fn begin_detection(&mut self, revision: u64) -> DetectionId {
        let id = DetectionId::new(self.epoch);
        self.pending = Some((id, revision));
        id
    }

    /// Explicit cancellation fences all existing evidence even if the model
    /// process cannot stop native inference immediately.
    pub fn invalidate(&mut self) -> Result<u64, EpochExhausted> {
        self.epoch = self.epoch.checked_add(1).ok_or(EpochExhausted)?;
        self.pending = None;
        Ok(self.epoch)
    }

    pub fn observe(
        &mut self,
        observation: &TurnObservation,
        current_revision: u64,
    ) -> Result<TurnDecision, EpochExhausted> {
        if !self.accept_evidence(observation, current_revision) {
            return Ok(TurnDecision::Stale);
        }
        Ok(match observation.state {
            TurnState::Complete if observation.transcript.trim().is_empty() => {
                TurnDecision::ContinueListening
            }
            TurnState::Complete => TurnDecision::Respond {
                generation_epoch: self.invalidate()?,
                transcript: observation.transcript.trim().to_string(),
            },
            TurnState::Incomplete => TurnDecision::ContinueListening,
            TurnState::Backchannel => TurnDecision::Backchannel,
            TurnState::Wait => {
                self.invalidate()?;
                TurnDecision::Wait
            }
        })
    }

    /// Consume a fresh decision without replacing its generation. Reversible
    /// output control interprets explicit pause/continue before deciding whether
    /// a genuinely new response requires `invalidate`.
    pub fn accept_evidence(&mut self, observation: &TurnObservation, revision: u64) -> bool {
        if self.pending != Some((observation.id, revision))
            || observation.id.generation_epoch != self.epoch
        {
            return false;
        }
        self.pending = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::ModelIdentity;

    fn evidence(id: DetectionId, state: TurnState, text: &str) -> TurnObservation {
        TurnObservation {
            id,
            state,
            transcript: text.into(),
            inference_ms: 1.0,
            model: ModelIdentity {
                source: "test".into(),
                revision: "test".into(),
                device: "test".into(),
            },
        }
    }

    #[test]
    fn incomplete_and_backchannel_keep_epoch_while_complete_advances_it() {
        let mut controller = TurnController::default();
        for (state, expected) in [
            (TurnState::Incomplete, TurnDecision::ContinueListening),
            (TurnState::Backchannel, TurnDecision::Backchannel),
        ] {
            let id = controller.begin_detection(1);
            assert_eq!(
                controller.observe(&evidence(id, state, "嗯"), 1).unwrap(),
                expected
            );
            assert_eq!(controller.generation_epoch(), 1);
        }
        let id = controller.begin_detection(2);
        assert_eq!(
            controller
                .observe(&evidence(id, TurnState::Complete, " 新的问题 "), 2)
                .unwrap(),
            TurnDecision::Respond {
                generation_epoch: 2,
                transcript: "新的问题".into()
            }
        );
    }

    #[test]
    fn cancelled_replaced_and_duplicate_evidence_cannot_restart_output() {
        let mut controller = TurnController::default();
        let old = controller.begin_detection(1);
        let new = controller.begin_detection(1);
        assert_eq!(
            controller
                .observe(&evidence(old, TurnState::Complete, "old"), 1)
                .unwrap(),
            TurnDecision::Stale
        );
        let result = evidence(new, TurnState::Complete, "new");
        assert!(matches!(
            controller.observe(&result, 1).unwrap(),
            TurnDecision::Respond { .. }
        ));
        assert_eq!(controller.observe(&result, 1).unwrap(), TurnDecision::Stale);
        let cancelled = controller.begin_detection(2);
        controller.invalidate().unwrap();
        assert_eq!(
            controller
                .observe(&evidence(cancelled, TurnState::Wait, "wait"), 2)
                .unwrap(),
            TurnDecision::Stale
        );
    }

    #[test]
    fn new_speech_during_inference_invalidates_the_incomplete_snapshot() {
        let mut controller = TurnController::default();
        let id = controller.begin_detection(1);
        assert_eq!(
            controller
                .observe(&evidence(id, TurnState::Complete, "first half"), 2)
                .unwrap(),
            TurnDecision::Stale
        );
        assert_eq!(controller.generation_epoch(), 1);
        let id = controller.begin_detection(2);
        assert_eq!(
            controller
                .observe(&evidence(id, TurnState::Complete, " "), 2)
                .unwrap(),
            TurnDecision::ContinueListening
        );
    }
    #[test]
    fn wait_advances_epoch_and_invalidates_prior_or_duplicate_evidence() {
        let mut controller = TurnController::default();
        let old = controller.begin_detection(1);
        let id = controller.begin_detection(2);
        let wait = evidence(id, TurnState::Wait, "wait");
        assert_eq!(controller.observe(&wait, 2).unwrap(), TurnDecision::Wait);
        assert_eq!(controller.generation_epoch(), 2);
        assert_eq!(controller.observe(&wait, 2).unwrap(), TurnDecision::Stale);
        assert_eq!(
            controller
                .observe(&evidence(old, TurnState::Complete, "old"), 1)
                .unwrap(),
            TurnDecision::Stale
        );
        let resumed = controller.begin_detection(3);
        assert!(matches!(
            controller
                .observe(&evidence(resumed, TurnState::Complete, "resume"), 3)
                .unwrap(),
            TurnDecision::Respond {
                generation_epoch: 3,
                ..
            }
        ));
    }

    #[test]
    fn wait_epoch_exhaustion_fails_without_wrapping() {
        let mut controller = TurnController {
            epoch: u64::MAX,
            pending: None,
        };
        let id = controller.begin_detection(1);
        assert_eq!(
            controller.observe(&evidence(id, TurnState::Wait, "wait"), 1),
            Err(EpochExhausted)
        );
        assert_eq!(controller.generation_epoch(), u64::MAX);
    }
}
