use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Immutable origin of voice output. Never fill it from a channel's current
/// session when old asynchronous work finally reaches the transport adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VoiceScope {
    pub device_boot_id: Uuid,
    pub session_id: Uuid,
    pub generation_epoch: u64,
}

impl VoiceScope {
    pub fn is_valid(&self) -> bool {
        !self.device_boot_id.is_nil() && !self.session_id.is_nil() && self.generation_epoch > 0
    }
}

/// Version 3 requires prior canonical output registration and independent
/// reversible control. Both peers must acknowledge it before sending media.
pub const PLAYBACK_CONTRACT_VERSION: u32 = 3;

/// Allocated before external I/O and never replaced during retry or cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlaybackIdentity {
    pub scope: VoiceScope,
    pub playback_id: Uuid,
    pub playback_sequence: u64,
}

impl PlaybackIdentity {
    pub fn is_valid(&self) -> bool {
        self.scope.is_valid() && !self.playback_id.is_nil() && self.playback_sequence > 0
    }
}

/// These boundaries prove submission, not DAC presentation or acoustic output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionBoundary {
    CpalCallbackBuffer,
    AlsaWriteAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionBackend {
    pub instance_id: Uuid,
    pub boundary: SubmissionBoundary,
    pub output_sample_rate_hz: u32,
    pub output_channels: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObservationEnd {
    SubmissionFinished,
    PresentedFinished,
    Cancelled { reason: String },
    Failed { reason: String },
    Unknown { reason: String },
    Rejected { reason: String },
}

/// Counts are source PCM frames after mean downmix, before resampling. One
/// stereo input frame therefore still counts once. None means no evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackProgress {
    pub identity: PlaybackIdentity,
    pub receipt_sequence: u64,
    pub accepted_source_frames: u64,
    pub submitted_source_frames: Option<u64>,
    pub presented_source_frames: Option<u64>,
    pub end_accepted: bool,
    pub backend: Option<SubmissionBackend>,
    pub observation_end: Option<ObservationEnd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid playback evidence: {0}")]
pub struct InvalidPlaybackEvidence(pub &'static str);

impl PlaybackProgress {
    /// Validate a strictly ordered receipt against the previous durable receipt
    /// and the durable offered bound. Deduplicating identical receipts belongs
    /// to the receiver, before this check. Terminal observations cannot revive.
    pub fn validate(
        &self,
        previous: Option<&Self>,
        offered_source_frames: u64,
    ) -> Result<(), InvalidPlaybackEvidence> {
        let require = |condition, reason| {
            if condition {
                Ok(())
            } else {
                Err(InvalidPlaybackEvidence(reason))
            }
        };
        require(self.identity.is_valid(), "invalid identity")?;
        require(self.receipt_sequence > 0, "zero receipt sequence")?;
        // An independently queried cancellation checkpoint may be the first
        // visible receipt after an ACK loss. Push itself separately requires
        // sequence one for the initial empty Start acknowledgement.
        require(
            self.accepted_source_frames <= offered_source_frames,
            "accepted exceeds offered",
        )?;
        if let Some(submitted) = self.submitted_source_frames {
            require(
                submitted <= self.accepted_source_frames,
                "submitted exceeds accepted",
            )?;
            require(self.backend.is_some(), "submission has no backend identity")?;
        }
        if let Some(presented) = self.presented_source_frames {
            require(
                self.submitted_source_frames
                    .is_some_and(|submitted| presented <= submitted),
                "presentation exceeds or lacks submission",
            )?;
        }
        if let Some(backend) = &self.backend {
            require(!backend.instance_id.is_nil(), "nil backend instance")?;
            require(
                (8000..=384000).contains(&backend.output_sample_rate_hz)
                    && (1..=32).contains(&backend.output_channels),
                "invalid backend format",
            )?;
        }
        if let Some(previous) = previous {
            require(self.identity == previous.identity, "identity changed")?;
            require(
                previous.observation_end.is_none(),
                "receipt after terminal observation",
            )?;
            require(
                self.receipt_sequence > previous.receipt_sequence,
                "receipt sequence regressed",
            )?;
            require(
                self.accepted_source_frames >= previous.accepted_source_frames,
                "acceptance regressed",
            )?;
            for (before, after) in [
                (
                    previous.submitted_source_frames,
                    self.submitted_source_frames,
                ),
                (
                    previous.presented_source_frames,
                    self.presented_source_frames,
                ),
            ] {
                require(
                    before.is_none_or(|value| after.is_some_and(|next| next >= value)),
                    "evidence regressed or disappeared",
                )?;
            }
            require(
                !previous.end_accepted || self.end_accepted,
                "End acceptance disappeared",
            )?;
            require(
                !previous.end_accepted
                    || previous.accepted_source_frames == self.accepted_source_frames,
                "audio accepted after End",
            )?;
            require(
                previous
                    .backend
                    .as_ref()
                    .is_none_or(|backend| Some(backend) == self.backend.as_ref()),
                "backend instance or format changed",
            )?;
        }
        if self.end_accepted {
            require(
                self.accepted_source_frames == offered_source_frames,
                "End does not cover all offered frames",
            )?;
        }
        match self.observation_end {
            Some(ObservationEnd::SubmissionFinished) => {
                require(
                    self.end_accepted
                        && self.submitted_source_frames == Some(self.accepted_source_frames),
                    "submission finished without complete evidence",
                )?;
            }
            Some(ObservationEnd::PresentedFinished) => {
                require(
                    self.end_accepted
                        && self.presented_source_frames == Some(self.accepted_source_frames),
                    "presentation finished without complete evidence",
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn presentation_finished(&self) -> bool {
        matches!(
            self.observation_end,
            Some(ObservationEnd::PresentedFinished)
        ) && self.end_accepted
            && self.presented_source_frames == Some(self.accepted_source_frames)
            && self.submitted_source_frames == Some(self.accepted_source_frames)
            && self.backend.is_some()
            && self.identity.is_valid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> PlaybackProgress {
        PlaybackProgress {
            identity: PlaybackIdentity {
                scope: VoiceScope {
                    device_boot_id: Uuid::from_u128(1),
                    session_id: Uuid::from_u128(2),
                    generation_epoch: 1,
                },
                playback_id: Uuid::from_u128(3),
                playback_sequence: 1,
            },
            receipt_sequence: 1,
            accepted_source_frames: 100,
            submitted_source_frames: Some(40),
            presented_source_frames: None,
            end_accepted: true,
            backend: Some(SubmissionBackend {
                instance_id: Uuid::from_u128(4),
                boundary: SubmissionBoundary::CpalCallbackBuffer,
                output_sample_rate_hz: 48000,
                output_channels: 2,
            }),
            observation_end: None,
        }
    }

    #[test]
    fn input_end_keeps_submission_observable_without_inventing_presentation() {
        let first = receipt();
        first.validate(None, 100).unwrap();
        let mut submitted = first.clone();
        submitted.receipt_sequence += 1;
        submitted.submitted_source_frames = Some(100);
        submitted.observation_end = Some(ObservationEnd::SubmissionFinished);
        submitted.validate(Some(&first), 100).unwrap();
        assert!(!submitted.presentation_finished());
        let mut late = submitted.clone();
        late.receipt_sequence += 1;
        assert!(late.validate(Some(&submitted), 100).is_err());
    }

    #[test]
    fn rejects_changed_identity_backend_and_regressed_or_impossible_counts() {
        let first = receipt();
        for mutation in 0..9 {
            let mut next = first.clone();
            next.receipt_sequence += 1;
            match mutation {
                0 => next.identity.scope.generation_epoch += 1,
                1 => next.accepted_source_frames = 101,
                2 => next.submitted_source_frames = Some(101),
                3 => next.presented_source_frames = Some(41),
                4 => next.submitted_source_frames = None,
                5 => next.backend.as_mut().unwrap().instance_id = Uuid::from_u128(99),
                6 => next.end_accepted = false,
                7 => next.observation_end = Some(ObservationEnd::PresentedFinished),
                8 => next.receipt_sequence = 0,
                _ => unreachable!(),
            }
            assert!(
                next.validate(Some(&first), 100).is_err(),
                "mutation {mutation}"
            );
        }
    }

    #[test]
    fn cancellation_keeps_the_known_submitted_prefix() {
        let first = receipt();
        let mut cancelled = first.clone();
        cancelled.receipt_sequence += 1;
        cancelled.observation_end = Some(ObservationEnd::Cancelled {
            reason: "scoped fence acknowledged".into(),
        });
        cancelled.validate(Some(&first), 100).unwrap();
        assert_eq!(cancelled.submitted_source_frames, Some(40));
        assert_eq!(cancelled.presented_source_frames, None);
        // A cancellation checkpoint may be the first observable receipt after
        // a lost Push ACK; its original device sequence must not be replaced.
        cancelled.receipt_sequence = 42;
        cancelled.validate(None, 100).unwrap();
        assert_eq!(cancelled.receipt_sequence, 42);
    }
}
