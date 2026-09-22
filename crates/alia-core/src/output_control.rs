//! Canonical binding and immutable wire intent for reversible audio control.
//! These values do not grant permission; both peers validate their original owner.
use crate::delivery::{PlaybackProgress, SubmissionBackend, VoiceScope};
use crate::playback_control::{
    ControlAction, ControlCommand, ControlPhase, ControlReceipt, MAX_CONTROL_COMMANDS, MAX_HOLD_MS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const VOICE_CONTRACT_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutputBinding {
    pub scope: VoiceScope,
    pub response_id: Uuid,
    pub context_id: Uuid,
    pub server_instance_id: Uuid,
}
impl OutputBinding {
    pub fn is_valid(&self) -> bool {
        self.scope.is_valid()
            && !self.response_id.is_nil()
            && !self.context_id.is_nil()
            && !self.server_instance_id.is_nil()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRegistration {
    pub binding: OutputBinding,
    pub backend: SubmissionBackend,
    pub maximum_hold_ms: u64,
}
impl OutputRegistration {
    pub fn validate(&self) -> Result<(), InvalidOutputControl> {
        require(self.binding.is_valid(), "invalid output binding")?;
        validate_backend(&self.backend)?;
        require(
            (1..=MAX_HOLD_MS).contains(&self.maximum_hold_ms),
            "invalid maximum hold",
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundControlCommand {
    pub binding: OutputBinding,
    pub command: ControlCommand,
    pub expected_backend: SubmissionBackend,
}
impl BoundControlCommand {
    pub fn validate(&self) -> Result<(), InvalidOutputControl> {
        require(self.binding.is_valid(), "invalid output binding")?;
        require(
            self.command.scope == self.binding.scope,
            "command scope differs from output",
        )?;
        require(
            !self.command.command_id.is_nil()
                && !self.command.hold_id.is_nil()
                && self.command.sequence > 0,
            "invalid command identity",
        )?;
        validate_backend(&self.expected_backend)?;
        match self.command.action {
            ControlAction::Pause { budget_ms, .. } => require(
                (1..=MAX_HOLD_MS).contains(&budget_ms),
                "hold budget outside contract",
            )?,
            ControlAction::Resume { authorization } => require(
                !authorization.detection_id.is_nil(),
                "resume has no detection identity",
            )?,
            ControlAction::ConfirmExplicitWait { detection_id, .. } => require(
                !detection_id.is_nil(),
                "wait confirmation has no detection identity",
            )?,
        }
        Ok(())
    }
    /// Fixed typed data has no arbitrary map or floating-point field. Every
    /// binding, command and backend field participates in the versioned intent.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = b"alia.playback.bound-control.v3\0".to_vec();
        bytes.extend(serde_json::to_vec(self).expect("fixed output control serializes"));
        bytes
    }
    pub fn intent_digest(&self) -> String {
        format!("{:x}", Sha256::digest(self.canonical_bytes()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlMode {
    SoftwareGate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundControlReceipt {
    /// Monotonic observation order on the original registered owner. This does
    /// not apply a command, renew a hold, or replace the immutable outcome.
    pub observation_sequence: u64,
    pub command: BoundControlCommand,
    pub intent_digest: String,
    pub receipt: ControlReceipt,
    pub backend: SubmissionBackend,
    pub mode: ControlMode,
    pub native_writes_quiescent: bool,
    pub progress: Vec<PlaybackProgress>,
}
impl BoundControlReceipt {
    pub fn validate(&self, expected: &BoundControlCommand) -> Result<(), InvalidOutputControl> {
        expected.validate()?;
        require(
            self.command == *expected && self.intent_digest == expected.intent_digest(),
            "control receipt changed original intent",
        )?;
        require(
            self.backend == expected.expected_backend,
            "control receipt changed native backend",
        )?;
        require(
            self.receipt.command_id == expected.command.command_id
                && self.receipt.snapshot.scope == expected.binding.scope,
            "control receipt identity changed",
        )?;
        require(
            self.observation_sequence > 0,
            "missing observation sequence",
        )?;
        let snapshot = &self.receipt.snapshot;
        require(
            self.receipt.outcome != crate::playback_control::CommandOutcome::Applied
                || snapshot.sequence >= expected.command.sequence,
            "applied command exceeds observed command watermark",
        )?;
        require(
            snapshot.remaining_ms.is_none_or(|ms| ms <= MAX_HOLD_MS),
            "remaining budget outside contract",
        )?;
        require(
            snapshot.hold_id.is_none_or(|id| !id.is_nil()),
            "nil hold identity",
        )?;
        require(
            (snapshot.phase == ControlPhase::Retired) == snapshot.retired.is_some(),
            "control retirement evidence mismatch",
        )?;
        match snapshot.phase {
            ControlPhase::Held | ControlPhase::Pausing => require(
                snapshot.hold_id.is_some()
                    && snapshot.reason.is_some()
                    && snapshot.remaining_ms.is_some(),
                "held state lacks hold identity",
            )?,
            ControlPhase::Running => require(
                snapshot.hold_id.is_none()
                    && snapshot.reason.is_none()
                    && snapshot.remaining_ms.is_none(),
                "running state retains a hold",
            )?,
            ControlPhase::Retired => (),
        }
        require(
            snapshot.phase != ControlPhase::Held || self.native_writes_quiescent,
            "Held has in-flight native writes",
        )?;
        require(
            self.progress.len() <= MAX_CONTROL_COMMANDS,
            "control evidence exceeds observation bound",
        )?;
        let mut identities = std::collections::HashSet::new();
        for progress in &self.progress {
            require(
                progress.identity.scope == expected.binding.scope
                    && identities.insert(progress.identity),
                "control evidence has foreign or duplicate playback",
            )?;
            require(
                progress
                    .backend
                    .as_ref()
                    .is_none_or(|backend| backend == &self.backend),
                "playback progress changed native backend",
            )?;
            progress
                .validate(None, progress.accepted_source_frames)
                .map_err(|_| InvalidOutputControl("invalid original playback progress"))?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid bound output control: {0}")]
pub struct InvalidOutputControl(pub &'static str);
fn require(condition: bool, reason: &'static str) -> Result<(), InvalidOutputControl> {
    if condition {
        Ok(())
    } else {
        Err(InvalidOutputControl(reason))
    }
}
pub fn validate_backend(backend: &SubmissionBackend) -> Result<(), InvalidOutputControl> {
    require(
        !backend.instance_id.is_nil()
            && (8000..=384000).contains(&backend.output_sample_rate_hz)
            && (1..=32).contains(&backend.output_channels),
        "invalid native backend",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::SubmissionBoundary;
    use crate::playback_control::HoldReason;
    fn command() -> BoundControlCommand {
        let scope = VoiceScope {
            device_boot_id: Uuid::from_u128(1),
            session_id: Uuid::from_u128(2),
            generation_epoch: 3,
        };
        BoundControlCommand {
            binding: OutputBinding {
                scope,
                response_id: Uuid::from_u128(4),
                context_id: Uuid::from_u128(5),
                server_instance_id: Uuid::from_u128(6),
            },
            command: ControlCommand {
                scope,
                hold_id: Uuid::from_u128(7),
                command_id: Uuid::from_u128(8),
                sequence: 1,
                action: ControlAction::Pause {
                    reason: HoldReason::AcousticCandidate,
                    through_sequence: 9,
                    budget_ms: MAX_HOLD_MS,
                },
            },
            expected_backend: SubmissionBackend {
                instance_id: Uuid::from_u128(10),
                boundary: SubmissionBoundary::CpalCallbackBuffer,
                output_sample_rate_hz: 48000,
                output_channels: 2,
            },
        }
    }
    #[test]
    fn immutable_digest_binds_original_output_and_backend_and_budget() {
        let original = command();
        original.validate().unwrap();
        assert_eq!(original.intent_digest(), original.clone().intent_digest());
        for change in 0..4 {
            let mut next = original.clone();
            match change {
                0 => next.binding.context_id = Uuid::from_u128(11),
                1 => next.expected_backend.output_channels = 1,
                2 => next.binding.server_instance_id = Uuid::from_u128(11),
                _ => {
                    if let ControlAction::Pause { budget_ms, .. } = &mut next.command.action {
                        *budget_ms -= 1;
                    }
                }
            }
            assert_ne!(original.intent_digest(), next.intent_digest());
        }
        let mut over = original;
        if let ControlAction::Pause { budget_ms, .. } = &mut over.command.action {
            *budget_ms += 1;
        }
        assert!(over.validate().is_err());
    }
}
