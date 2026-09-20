//! Strict conversion for the paired playback v3 / voice v2 contract.
use crate::playback as wire;
use alia_core::delivery::{
    self, ObservationEnd, PlaybackIdentity, PlaybackProgress, SubmissionBackend,
    SubmissionBoundary, VoiceScope,
};
use alia_core::output_control::{
    BoundControlCommand, BoundControlReceipt, ControlMode, OutputBinding, OutputRegistration,
};
use alia_core::playback_control::{
    CommandOutcome, ControlAction, ControlCommand, ControlPhase, ControlReceipt, ControlRejection,
    ControlSnapshot, HoldReason, ResumeAuthorization, ResumeKind, RetiredReason,
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlCodecError(pub &'static str);
impl std::fmt::Display for ControlCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ControlCodecError {}
type Result<T> = std::result::Result<T, ControlCodecError>;
fn needed<T>(value: Option<T>, name: &'static str) -> Result<T> {
    value.ok_or(ControlCodecError(name))
}
fn check(ok: bool, message: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(ControlCodecError(message))
    }
}
fn uuid(value: &str) -> Result<Uuid> {
    let id = Uuid::parse_str(value).map_err(|_| ControlCodecError("invalid UUID"))?;
    check(
        !id.is_nil() && id.to_string() == value,
        "noncanonical or nil UUID",
    )?;
    Ok(id)
}

pub fn encode_scope(value: VoiceScope) -> wire::VoiceScope {
    wire::VoiceScope {
        device_boot_id: value.device_boot_id.to_string(),
        session_id: value.session_id.to_string(),
        generation_epoch: value.generation_epoch,
    }
}
pub fn decode_scope(value: &wire::VoiceScope) -> Result<VoiceScope> {
    let scope = VoiceScope {
        device_boot_id: uuid(&value.device_boot_id)?,
        session_id: uuid(&value.session_id)?,
        generation_epoch: value.generation_epoch,
    };
    check(scope.is_valid(), "invalid voice scope")?;
    Ok(scope)
}
pub fn encode_backend(value: &SubmissionBackend) -> wire::SubmissionBackend {
    wire::SubmissionBackend {
        instance_id: value.instance_id.to_string(),
        boundary: match value.boundary {
            SubmissionBoundary::CpalCallbackBuffer => wire::SubmissionBoundary::CpalCallbackBuffer,
            SubmissionBoundary::AlsaWriteAccepted => wire::SubmissionBoundary::AlsaWriteAccepted,
        } as i32,
        output_sample_rate_hz: value.output_sample_rate_hz,
        output_channels: value.output_channels,
    }
}
pub fn decode_backend(value: &wire::SubmissionBackend) -> Result<SubmissionBackend> {
    let result = SubmissionBackend {
        instance_id: uuid(&value.instance_id)?,
        boundary: match wire::SubmissionBoundary::try_from(value.boundary) {
            Ok(wire::SubmissionBoundary::CpalCallbackBuffer) => {
                SubmissionBoundary::CpalCallbackBuffer
            }
            Ok(wire::SubmissionBoundary::AlsaWriteAccepted) => {
                SubmissionBoundary::AlsaWriteAccepted
            }
            _ => return Err(ControlCodecError("unknown submission boundary")),
        },
        output_sample_rate_hz: value.output_sample_rate_hz,
        output_channels: value.output_channels,
    };
    alia_core::output_control::validate_backend(&result).map_err(|e| ControlCodecError(e.0))?;
    Ok(result)
}
pub fn encode_binding(value: OutputBinding) -> wire::OutputBinding {
    wire::OutputBinding {
        scope: Some(encode_scope(value.scope)),
        response_id: value.response_id.to_string(),
        context_id: value.context_id.to_string(),
        server_instance_id: value.server_instance_id.to_string(),
    }
}
pub fn decode_binding(value: &wire::OutputBinding) -> Result<OutputBinding> {
    Ok(OutputBinding {
        scope: decode_scope(needed(value.scope.as_ref(), "missing binding scope")?)?,
        response_id: uuid(&value.response_id)?,
        context_id: uuid(&value.context_id)?,
        server_instance_id: uuid(&value.server_instance_id)?,
    })
}
pub fn encode_registration(value: &OutputRegistration) -> wire::RegisterOutputReceipt {
    wire::RegisterOutputReceipt {
        binding: Some(encode_binding(value.binding)),
        backend: Some(encode_backend(&value.backend)),
        maximum_hold_ms: value.maximum_hold_ms,
        contract_version: delivery::PLAYBACK_CONTRACT_VERSION,
        software_hold: true,
    }
}
pub fn decode_registration(value: &wire::RegisterOutputReceipt) -> Result<OutputRegistration> {
    check(
        value.contract_version == delivery::PLAYBACK_CONTRACT_VERSION && value.software_hold,
        "incompatible output registration",
    )?;
    let result = OutputRegistration {
        binding: decode_binding(needed(
            value.binding.as_ref(),
            "missing registered binding",
        )?)?,
        backend: decode_backend(needed(
            value.backend.as_ref(),
            "missing registered backend",
        )?)?,
        maximum_hold_ms: value.maximum_hold_ms,
    };
    result.validate().map_err(|e| ControlCodecError(e.0))?;
    Ok(result)
}
fn encode_reason(reason: HoldReason) -> i32 {
    (match reason {
        HoldReason::AcousticCandidate => wire::HoldReason::AcousticCandidate,
        HoldReason::ExplicitWait => wire::HoldReason::ExplicitWait,
    }) as i32
}
fn decode_reason(reason: i32) -> Result<HoldReason> {
    match wire::HoldReason::try_from(reason) {
        Ok(wire::HoldReason::AcousticCandidate) => Ok(HoldReason::AcousticCandidate),
        Ok(wire::HoldReason::ExplicitWait) => Ok(HoldReason::ExplicitWait),
        _ => Err(ControlCodecError("unknown hold reason")),
    }
}
pub fn encode_command(value: &BoundControlCommand) -> wire::BoundControlCommand {
    use wire::control_command::Action;
    let action = match value.command.action {
        ControlAction::Pause {
            reason,
            through_sequence,
            budget_ms,
        } => Action::Pause(wire::PauseCommand {
            reason: encode_reason(reason),
            through_sequence,
            budget_ms,
        }),
        ControlAction::Resume { authorization: a } => Action::Resume(wire::ResumeCommand {
            kind: match a.kind {
                ResumeKind::FreshBackchannel => wire::ResumeKind::FreshBackchannel,
                ResumeKind::ExplicitContinue => wire::ResumeKind::ExplicitContinue,
            } as i32,
            detection_id: a.detection_id.to_string(),
            through_sequence: a.through_sequence,
        }),
        ControlAction::ConfirmExplicitWait {
            detection_id,
            through_sequence,
        } => Action::ConfirmExplicitWait(wire::ConfirmExplicitWaitCommand {
            detection_id: detection_id.to_string(),
            through_sequence,
        }),
    };
    wire::BoundControlCommand {
        binding: Some(encode_binding(value.binding)),
        command: Some(wire::ControlCommand {
            scope: Some(encode_scope(value.command.scope)),
            hold_id: value.command.hold_id.to_string(),
            command_id: value.command.command_id.to_string(),
            sequence: value.command.sequence,
            action: Some(action),
        }),
        expected_backend: Some(encode_backend(&value.expected_backend)),
    }
}
pub fn decode_command(value: &wire::BoundControlCommand) -> Result<BoundControlCommand> {
    use wire::control_command::Action;
    let command = needed(value.command.as_ref(), "missing control command")?;
    let action = match needed(command.action.as_ref(), "missing control action")? {
        Action::Pause(p) => ControlAction::Pause {
            reason: decode_reason(p.reason)?,
            through_sequence: p.through_sequence,
            budget_ms: p.budget_ms,
        },
        Action::Resume(r) => ControlAction::Resume {
            authorization: ResumeAuthorization {
                kind: match wire::ResumeKind::try_from(r.kind) {
                    Ok(wire::ResumeKind::FreshBackchannel) => ResumeKind::FreshBackchannel,
                    Ok(wire::ResumeKind::ExplicitContinue) => ResumeKind::ExplicitContinue,
                    _ => return Err(ControlCodecError("unknown resume authorization")),
                },
                detection_id: uuid(&r.detection_id)?,
                through_sequence: r.through_sequence,
            },
        },
        Action::ConfirmExplicitWait(c) => ControlAction::ConfirmExplicitWait {
            detection_id: uuid(&c.detection_id)?,
            through_sequence: c.through_sequence,
        },
    };
    let result = BoundControlCommand {
        binding: decode_binding(needed(value.binding.as_ref(), "missing output binding")?)?,
        command: ControlCommand {
            scope: decode_scope(needed(command.scope.as_ref(), "missing command scope")?)?,
            hold_id: uuid(&command.hold_id)?,
            command_id: uuid(&command.command_id)?,
            sequence: command.sequence,
            action,
        },
        expected_backend: decode_backend(needed(
            value.expected_backend.as_ref(),
            "missing expected backend",
        )?)?,
    };
    result.validate().map_err(|e| ControlCodecError(e.0))?;
    Ok(result)
}
pub fn encode_control_request(command: &BoundControlCommand) -> wire::ControlRequest {
    wire::ControlRequest {
        command: Some(encode_command(command)),
        intent_digest: command.intent_digest(),
    }
}
pub fn decode_control_request(request: &wire::ControlRequest) -> Result<BoundControlCommand> {
    let command = decode_command(needed(request.command.as_ref(), "missing bound command")?)?;
    check(
        request.intent_digest == command.intent_digest(),
        "control intent digest mismatch",
    )?;
    Ok(command)
}
fn encode_rejection(value: ControlRejection) -> i32 {
    (match value {
        ControlRejection::Invalid => wire::ControlRejection::Invalid,
        ControlRejection::IdentityConflict => wire::ControlRejection::IdentityConflict,
        ControlRejection::Sequence => wire::ControlRejection::Sequence,
        ControlRejection::AnotherHold => wire::ControlRejection::AnotherHold,
        ControlRejection::NotHeld => wire::ControlRejection::NotHeld,
        ControlRejection::StaleEvidence => wire::ControlRejection::StaleEvidence,
        ControlRejection::ExplicitContinueRequired => {
            wire::ControlRejection::ExplicitContinueRequired
        }
        ControlRejection::NativeWritePending => wire::ControlRejection::NativeWritePending,
        ControlRejection::Retired => wire::ControlRejection::Retired,
    }) as i32
}
fn decode_rejection(value: i32) -> Result<ControlRejection> {
    Ok(match wire::ControlRejection::try_from(value) {
        Ok(wire::ControlRejection::Invalid) => ControlRejection::Invalid,
        Ok(wire::ControlRejection::IdentityConflict) => ControlRejection::IdentityConflict,
        Ok(wire::ControlRejection::Sequence) => ControlRejection::Sequence,
        Ok(wire::ControlRejection::AnotherHold) => ControlRejection::AnotherHold,
        Ok(wire::ControlRejection::NotHeld) => ControlRejection::NotHeld,
        Ok(wire::ControlRejection::StaleEvidence) => ControlRejection::StaleEvidence,
        Ok(wire::ControlRejection::ExplicitContinueRequired) => {
            ControlRejection::ExplicitContinueRequired
        }
        Ok(wire::ControlRejection::NativeWritePending) => ControlRejection::NativeWritePending,
        Ok(wire::ControlRejection::Retired) => ControlRejection::Retired,
        _ => return Err(ControlCodecError("unknown control rejection")),
    })
}
fn encode_retired(value: RetiredReason) -> i32 {
    (match value {
        RetiredReason::Cancelled => wire::RetiredReason::Cancelled,
        RetiredReason::Closed => wire::RetiredReason::Closed,
        RetiredReason::Expired => wire::RetiredReason::Expired,
        RetiredReason::BackendChanged => wire::RetiredReason::BackendChanged,
        RetiredReason::Capacity => wire::RetiredReason::Capacity,
    }) as i32
}
fn decode_retired(value: i32) -> Result<RetiredReason> {
    match wire::RetiredReason::try_from(value) {
        Ok(wire::RetiredReason::Cancelled) => Ok(RetiredReason::Cancelled),
        Ok(wire::RetiredReason::Closed) => Ok(RetiredReason::Closed),
        Ok(wire::RetiredReason::Expired) => Ok(RetiredReason::Expired),
        Ok(wire::RetiredReason::BackendChanged) => Ok(RetiredReason::BackendChanged),
        Ok(wire::RetiredReason::Capacity) => Ok(RetiredReason::Capacity),
        _ => Err(ControlCodecError("unknown retirement reason")),
    }
}
pub fn encode_snapshot(value: &ControlSnapshot) -> wire::ControlSnapshot {
    wire::ControlSnapshot {
        scope: Some(encode_scope(value.scope)),
        phase: (match value.phase {
            ControlPhase::Running => wire::ControlPhase::Running,
            ControlPhase::Pausing => wire::ControlPhase::Pausing,
            ControlPhase::Held => wire::ControlPhase::Held,
            ControlPhase::Retired => wire::ControlPhase::Retired,
        }) as i32,
        hold_id: value.hold_id.map(|id| id.to_string()),
        reason: value.reason.map(encode_reason),
        sequence: value.sequence,
        remaining_ms: value.remaining_ms,
        retired: value.retired.map(encode_retired),
    }
}
pub fn decode_snapshot(value: &wire::ControlSnapshot) -> Result<ControlSnapshot> {
    Ok(ControlSnapshot {
        scope: decode_scope(needed(value.scope.as_ref(), "missing snapshot scope")?)?,
        phase: match wire::ControlPhase::try_from(value.phase) {
            Ok(wire::ControlPhase::Running) => ControlPhase::Running,
            Ok(wire::ControlPhase::Pausing) => ControlPhase::Pausing,
            Ok(wire::ControlPhase::Held) => ControlPhase::Held,
            Ok(wire::ControlPhase::Retired) => ControlPhase::Retired,
            _ => return Err(ControlCodecError("unknown control phase")),
        },
        hold_id: value.hold_id.as_deref().map(uuid).transpose()?,
        reason: value.reason.map(decode_reason).transpose()?,
        sequence: value.sequence,
        remaining_ms: value.remaining_ms,
        retired: value.retired.map(decode_retired).transpose()?,
    })
}
pub fn encode_progress(value: &PlaybackProgress) -> wire::PlaybackReceipt {
    use wire::{ObservationEnd as E, PlaybackState as S};
    let (end, state, reason) = match &value.observation_end {
        None => (E::Unspecified, S::Accepted, String::new()),
        Some(ObservationEnd::SubmissionFinished) => {
            (E::SubmissionFinished, S::Submitted, String::new())
        }
        Some(ObservationEnd::PresentedFinished) => {
            (E::PresentedFinished, S::Completed, String::new())
        }
        Some(ObservationEnd::Cancelled { reason }) => (E::Cancelled, S::Cancelled, reason.clone()),
        Some(ObservationEnd::Failed { reason }) => (E::Failed, S::Failed, reason.clone()),
        Some(ObservationEnd::Unknown { reason }) => (E::Unknown, S::Unknown, reason.clone()),
        Some(ObservationEnd::Rejected { reason }) => (E::Rejected, S::Rejected, reason.clone()),
    };
    wire::PlaybackReceipt {
        scope: Some(encode_scope(value.identity.scope)),
        playback_id: value.identity.playback_id.to_string(),
        playback_sequence: value.identity.playback_sequence,
        state: state as i32,
        accepted_samples: value.accepted_source_frames,
        end_accepted: value.end_accepted,
        submitted_samples: value.submitted_source_frames,
        presented_samples: value.presented_source_frames,
        contract_version: delivery::PLAYBACK_CONTRACT_VERSION,
        receipt_sequence: value.receipt_sequence,
        backend: value.backend.as_ref().map(encode_backend),
        observation_end: end as i32,
        reason,
    }
}
pub fn decode_progress(value: &wire::PlaybackReceipt) -> Result<PlaybackProgress> {
    check(
        value.contract_version == delivery::PLAYBACK_CONTRACT_VERSION,
        "old playback evidence contract",
    )?;
    check(
        value.presented_samples.is_none(),
        "physical presentation is not implemented",
    )?;
    use wire::{ObservationEnd as E, PlaybackState as S};
    let (observation_end, state) = match E::try_from(value.observation_end) {
        Ok(E::Unspecified) => (None, S::Accepted),
        Ok(E::SubmissionFinished) => (Some(ObservationEnd::SubmissionFinished), S::Submitted),
        Ok(E::Cancelled) => (
            Some(ObservationEnd::Cancelled {
                reason: value.reason.clone(),
            }),
            S::Cancelled,
        ),
        Ok(E::Failed) => (
            Some(ObservationEnd::Failed {
                reason: value.reason.clone(),
            }),
            S::Failed,
        ),
        Ok(E::Unknown) => (
            Some(ObservationEnd::Unknown {
                reason: value.reason.clone(),
            }),
            S::Unknown,
        ),
        Ok(E::Rejected) => (
            Some(ObservationEnd::Rejected {
                reason: value.reason.clone(),
            }),
            S::Rejected,
        ),
        _ => {
            return Err(ControlCodecError(
                "unknown or unimplemented observation end",
            ));
        }
    };
    check(
        value.state == state as i32,
        "playback state and observation end disagree",
    )?;
    let result = PlaybackProgress {
        identity: PlaybackIdentity {
            scope: decode_scope(needed(value.scope.as_ref(), "missing progress scope")?)?,
            playback_id: uuid(&value.playback_id)?,
            playback_sequence: value.playback_sequence,
        },
        receipt_sequence: value.receipt_sequence,
        accepted_source_frames: value.accepted_samples,
        submitted_source_frames: value.submitted_samples,
        presented_source_frames: value.presented_samples,
        end_accepted: value.end_accepted,
        backend: value.backend.as_ref().map(decode_backend).transpose()?,
        observation_end,
    };
    result
        .validate(None, result.accepted_source_frames)
        .map_err(|_| ControlCodecError("invalid playback checkpoint"))?;
    Ok(result)
}
pub fn encode_control_receipt(value: &BoundControlReceipt) -> wire::ControlResponse {
    wire::ControlResponse {
        observation_sequence: value.observation_sequence,
        command: Some(encode_command(&value.command)),
        intent_digest: value.intent_digest.clone(),
        receipt: Some(wire::ControlReceipt {
            command_id: value.receipt.command_id.to_string(),
            outcome: Some(match value.receipt.outcome {
                CommandOutcome::Applied => wire::control_receipt::Outcome::Applied(true),
                CommandOutcome::Rejected(reason) => {
                    wire::control_receipt::Outcome::Rejected(encode_rejection(reason))
                }
            }),
            snapshot: Some(encode_snapshot(&value.receipt.snapshot)),
        }),
        backend: Some(encode_backend(&value.backend)),
        mode: wire::ControlMode::SoftwareGate as i32,
        native_writes_quiescent: value.native_writes_quiescent,
        progress: value.progress.iter().map(encode_progress).collect(),
    }
}
pub fn decode_control_receipt(
    value: &wire::ControlResponse,
    expected: &BoundControlCommand,
) -> Result<BoundControlReceipt> {
    check(
        value.mode == wire::ControlMode::SoftwareGate as i32,
        "unsupported playback control mode",
    )?;
    check(
        value.progress.len() <= alia_core::playback_control::MAX_CONTROL_COMMANDS,
        "too many control checkpoints",
    )?;
    let receipt = needed(value.receipt.as_ref(), "missing control outcome")?;
    let result = BoundControlReceipt {
        observation_sequence: value.observation_sequence,
        command: decode_command(needed(value.command.as_ref(), "missing receipt command")?)?,
        intent_digest: value.intent_digest.clone(),
        receipt: ControlReceipt {
            command_id: uuid(&receipt.command_id)?,
            outcome: match needed(receipt.outcome.as_ref(), "missing command outcome")? {
                wire::control_receipt::Outcome::Applied(true) => CommandOutcome::Applied,
                wire::control_receipt::Outcome::Rejected(reason) => {
                    CommandOutcome::Rejected(decode_rejection(*reason)?)
                }
                _ => return Err(ControlCodecError("false Applied is not an outcome")),
            },
            snapshot: decode_snapshot(needed(
                receipt.snapshot.as_ref(),
                "missing current snapshot",
            )?)?,
        },
        backend: decode_backend(needed(value.backend.as_ref(), "missing control backend")?)?,
        mode: ControlMode::SoftwareGate,
        native_writes_quiescent: value.native_writes_quiescent,
        progress: value
            .progress
            .iter()
            .map(decode_progress)
            .collect::<Result<Vec<_>>>()?,
    };
    result
        .validate(expected)
        .map_err(|e| ControlCodecError(e.0))?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alia_core::playback_control::MAX_HOLD_MS;
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
                    reason: HoldReason::ExplicitWait,
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
    fn all_commands_roundtrip_with_one_canonical_intent() {
        let mut original = command();
        for action in [
            original.command.action.clone(),
            ControlAction::Resume {
                authorization: ResumeAuthorization {
                    kind: ResumeKind::ExplicitContinue,
                    detection_id: Uuid::from_u128(11),
                    through_sequence: 12,
                },
            },
            ControlAction::ConfirmExplicitWait {
                detection_id: Uuid::from_u128(13),
                through_sequence: 14,
            },
        ] {
            original.command.action = action;
            let wire = encode_control_request(&original);
            assert_eq!(decode_control_request(&wire).unwrap(), original);
            let mut changed = wire.clone();
            changed
                .command
                .as_mut()
                .unwrap()
                .expected_backend
                .as_mut()
                .unwrap()
                .output_channels = 1;
            assert!(decode_control_request(&changed).is_err());
            let mut missing = wire;
            missing
                .command
                .as_mut()
                .unwrap()
                .binding
                .as_mut()
                .unwrap()
                .context_id
                .clear();
            assert!(decode_control_request(&missing).is_err());
        }
    }
    #[test]
    fn receipt_distinguishes_original_outcome_from_current_retirement() {
        let command = command();
        let value = BoundControlReceipt {
            observation_sequence: 2,
            command: command.clone(),
            intent_digest: command.intent_digest(),
            backend: command.expected_backend.clone(),
            mode: ControlMode::SoftwareGate,
            native_writes_quiescent: true,
            progress: vec![],
            receipt: ControlReceipt {
                command_id: command.command.command_id,
                outcome: CommandOutcome::Applied,
                snapshot: ControlSnapshot {
                    scope: command.binding.scope,
                    phase: ControlPhase::Retired,
                    hold_id: Some(command.command.hold_id),
                    reason: Some(HoldReason::ExplicitWait),
                    sequence: 1,
                    remaining_ms: Some(0),
                    retired: Some(RetiredReason::Expired),
                },
            },
        };
        let wire = encode_control_receipt(&value);
        assert_eq!(decode_control_receipt(&wire, &command).unwrap(), value);
        let mut missing = wire.clone();
        missing.observation_sequence = 0;
        assert!(decode_control_receipt(&missing, &command).is_err());
        let mut invented = wire;
        invented
            .receipt
            .as_mut()
            .unwrap()
            .snapshot
            .as_mut()
            .unwrap()
            .phase = wire::ControlPhase::Held as i32;
        assert!(decode_control_receipt(&invented, &command).is_err());
    }
    #[test]
    fn capabilities_and_unknown_enums_fail_closed() {
        let command = command();
        let registration = OutputRegistration {
            binding: command.binding,
            backend: command.expected_backend.clone(),
            maximum_hold_ms: MAX_HOLD_MS,
        };
        let mut wire = encode_registration(&registration);
        assert_eq!(decode_registration(&wire).unwrap(), registration);
        wire.contract_version = 2;
        assert!(decode_registration(&wire).is_err());
        wire.contract_version = delivery::PLAYBACK_CONTRACT_VERSION;
        wire.maximum_hold_ms += 1;
        assert!(decode_registration(&wire).is_err());
        let mut wire = encode_command(&command);
        wire.expected_backend.as_mut().unwrap().boundary = 99;
        assert!(decode_command(&wire).is_err());
    }
}
