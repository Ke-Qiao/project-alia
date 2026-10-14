//! Device body commands and evidence. Identity and software submission are
//! independent of the transport and of any particular servo driver.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::delivery::VoiceScope;

pub const BODY_CONTRACT_VERSION: u32 = 2;
pub const MAX_BODY_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_BODY_ACTIONS: usize = 32;
pub const MAX_BODY_DURATION_MS: u32 = 5_000;
pub const MAX_BODY_CHANNELS: usize = 32;
pub const MAX_BODY_FRAMES: u64 = 3600;
pub const MAX_BODY_ANIMATION_BYTES: u32 = 65_536;
pub const MAX_BODY_ANIMATION_ASSETS: usize = 64;

/// Operator-approved content identity, never inferred from a model or filename.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyAnimationAsset {
    pub file: String,
    pub version: u32,
    pub sha256: String,
    pub bytes: u32,
}

impl BodyAnimationAsset {
    pub fn validate(&self) -> Result<(), InvalidBodyEvidence> {
        require(
            self.file.len() > 5
                && self.file.len() <= 128
                && self.file.ends_with(".anim")
                && self
                    .file
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')),
            "invalid canonical animation file name",
        )?;
        require(self.version > 0, "invalid animation asset version")?;
        require(
            self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid animation SHA-256",
        )?;
        require(
            (1..=MAX_BODY_ANIMATION_BYTES).contains(&self.bytes),
            "invalid animation byte count",
        )
    }

    pub fn verify_bytes(&self, content: &[u8]) -> Result<(), InvalidBodyEvidence> {
        self.validate()?;
        require(
            content.len() == self.bytes as usize,
            "animation byte count mismatch",
        )?;
        require(
            body_animation_sha256(content) == self.sha256,
            "animation SHA-256 mismatch",
        )
    }
}

/// Pure digest calculation for explicit tooling; it does not approve an asset.
pub fn body_animation_sha256(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

pub fn validate_animation_catalog(
    assets: &[BodyAnimationAsset],
) -> Result<(), InvalidBodyEvidence> {
    require(
        assets.len() <= MAX_BODY_ANIMATION_ASSETS,
        "animation catalog exceeds capacity",
    )?;
    let mut names = std::collections::HashSet::new();
    for asset in assets {
        asset.validate()?;
        require(
            names.insert(&asset.file),
            "duplicate approved animation file",
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyControlLease {
    pub device_boot_id: Uuid,
    pub server_instance_id: Uuid,
    pub generation: u64,
}

impl BodyControlLease {
    pub fn is_valid(&self) -> bool {
        !self.device_boot_id.is_nil() && !self.server_instance_id.is_nil() && self.generation > 0
    }
}

/// This tuple is the command ID. A caller cannot change a separate UUID while
/// reusing an old sequence, or re-sign the same ID under a new generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyCommandIdentity {
    pub lease: BodyControlLease,
    pub sequence: u64,
}

impl BodyCommandIdentity {
    pub fn is_valid(&self) -> bool {
        self.lease.is_valid() && self.sequence > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BodyScope {
    Voice { scope: VoiceScope },
    Nonvoice { scope_id: Uuid },
}

impl BodyScope {
    pub fn is_valid(&self, boot: Uuid) -> bool {
        match self {
            Self::Voice { scope } => scope.is_valid() && scope.device_boot_id == boot,
            Self::Nonvoice { scope_id } => !scope_id.is_nil(),
        }
    }

    pub fn voice(&self) -> Option<VoiceScope> {
        match self {
            Self::Voice { scope } => Some(*scope),
            Self::Nonvoice { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyPriority {
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BodyTarget {
    Rotation {
        degrees: [f32; 3],
    },
    FaceCenter {
        cx: f32,
        cy: f32,
        width: f32,
        height: f32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BodyMotion {
    SmallNod,
    SmallBow,
    LookAt {
        target: BodyTarget,
    },
    Animation {
        asset: BodyAnimationAsset,
        fps: u32,
        loops: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyAction {
    pub duration_ms: u32,
    pub intensity: f32,
    pub motion: BodyMotion,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyCommand {
    pub identity: BodyCommandIdentity,
    pub plan_id: Uuid,
    /// Immutable caller correlation, distinct from the device command identity.
    pub source_command_id: String,
    pub owner_id: Uuid,
    pub scope: BodyScope,
    pub priority: BodyPriority,
    pub interruptible: bool,
    pub actions: Vec<BodyAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid body command or evidence: {0}")]
pub struct InvalidBodyEvidence(pub &'static str);

fn require(value: bool, reason: &'static str) -> Result<(), InvalidBodyEvidence> {
    if value {
        Ok(())
    } else {
        Err(InvalidBodyEvidence(reason))
    }
}

impl BodyCommand {
    /// Transport-independent bounds. The device must additionally validate all
    /// calibration, animation file contents and generated frames before submit.
    pub fn validate(&self) -> Result<(), InvalidBodyEvidence> {
        require(self.identity.is_valid(), "invalid identity")?;
        require(
            !self.plan_id.is_nil() && !self.owner_id.is_nil(),
            "nil owner or plan",
        )?;
        require(
            !self.source_command_id.trim().is_empty() && self.source_command_id.len() <= 256,
            "invalid source command",
        )?;
        require(
            self.scope.is_valid(self.identity.lease.device_boot_id),
            "invalid original scope",
        )?;
        require(
            !self.actions.is_empty() && self.actions.len() <= MAX_BODY_ACTIONS,
            "invalid action count",
        )?;
        let mut total = 0_u32;
        for action in &self.actions {
            require(
                action.intensity.is_finite() && (0.0..=1.0).contains(&action.intensity),
                "invalid intensity",
            )?;
            require(
                (1..=MAX_BODY_DURATION_MS).contains(&action.duration_ms),
                "invalid action duration",
            )?;
            total = total
                .checked_add(action.duration_ms)
                .ok_or(InvalidBodyEvidence("duration overflow"))?;
            match &action.motion {
                BodyMotion::LookAt {
                    target: BodyTarget::Rotation { degrees },
                } => {
                    require(
                        degrees.iter().all(|v| v.is_finite() && v.abs() <= 180.0),
                        "invalid rotation",
                    )?;
                }
                BodyMotion::LookAt {
                    target:
                        BodyTarget::FaceCenter {
                            cx,
                            cy,
                            width,
                            height,
                        },
                } => {
                    require(
                        [cx, cy, width, height].iter().all(|v| v.is_finite()),
                        "nonfinite face target",
                    )?;
                    require(
                        *width > 0.0
                            && *height > 0.0
                            && *width <= 65536.0
                            && *height <= 65536.0
                            && (0.0..=*width).contains(cx)
                            && (0.0..=*height).contains(cy),
                        "invalid face target",
                    )?;
                }
                BodyMotion::Animation { asset, fps, loops } => {
                    asset.validate()?;
                    require(
                        (1..=120).contains(fps) && (1..=20).contains(loops),
                        "invalid animation playback bounds",
                    )?;
                }
                BodyMotion::SmallNod | BodyMotion::SmallBow => {}
            }
        }
        require(
            total <= MAX_BODY_DURATION_MS,
            "timeline exceeds duration bound",
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodySubmissionBoundary {
    SerialHostWrite,
    I2cRegisterWrite,
    Simulated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyBackend {
    pub instance_id: Uuid,
    pub boundary: BodySubmissionBoundary,
    pub channels: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BodyEnd {
    Submitted,
    Cancelled { reason: String },
    Failed { reason: String },
    Unknown { reason: String },
    Rejected { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyProgress {
    pub identity: BodyCommandIdentity,
    pub receipt_sequence: u64,
    pub accepted_actions: u32,
    pub started_actions: u32,
    pub submitted_actions: u32,
    /// Complete servo frames submitted, never audio frames or measured positions.
    pub submitted_frames: u64,
    /// Zero-based next frame when only some channels were written.
    pub partial_frame: Option<u64>,
    pub last_partial_channels: Vec<u32>,
    /// A write was attempted but the backend cannot prove its full submission.
    pub last_uncertain_channels: Vec<u32>,
    pub backend: Option<BodyBackend>,
    pub writer_quiescent: bool,
    /// All v1 backends are open-loop; these must remain unknown.
    pub position_confirmed_actions: Option<u32>,
    pub end: Option<BodyEnd>,
}

impl BodyProgress {
    pub fn validate(
        &self,
        previous: Option<&Self>,
        command: &BodyCommand,
    ) -> Result<(), InvalidBodyEvidence> {
        command.validate()?;
        require(
            self.identity == command.identity && self.receipt_sequence > 0,
            "receipt identity or sequence mismatch",
        )?;
        require(
            self.accepted_actions <= command.actions.len() as u32
                && self.started_actions <= self.accepted_actions
                && self.submitted_actions <= self.started_actions,
            "action evidence exceeds admitted bounds",
        )?;
        require(
            self.position_confirmed_actions.is_none(),
            "open-loop backend has no position evidence",
        )?;
        require(
            self.submitted_frames <= MAX_BODY_FRAMES,
            "submission exceeds bounded native plan",
        )?;
        require(
            self.submitted_actions == 0 || self.submitted_frames > 0,
            "action submitted without frame evidence",
        )?;
        let partial =
            !self.last_partial_channels.is_empty() || !self.last_uncertain_channels.is_empty();
        require(
            self.partial_frame.is_some() == partial,
            "partial frame evidence mismatch",
        )?;
        require(
            self.partial_frame
                .is_none_or(|frame| frame == self.submitted_frames),
            "partial frame is not next uncommitted frame",
        )?;
        if self.submitted_frames > 0 || partial {
            require(self.backend.is_some(), "submission has no backend identity")?;
            require(
                self.started_actions > 0,
                "submission without a started action",
            )?;
        }
        if let Some(backend) = &self.backend {
            require(
                !backend.instance_id.is_nil()
                    && (1..=MAX_BODY_CHANNELS as u32).contains(&backend.channels),
                "invalid backend",
            )?;
            let mut seen = [false; MAX_BODY_CHANNELS];
            for &channel in self
                .last_partial_channels
                .iter()
                .chain(&self.last_uncertain_channels)
            {
                require(
                    channel < backend.channels && !seen[channel as usize],
                    "duplicate or invalid partial channel",
                )?;
                seen[channel as usize] = true;
            }
        }
        if let Some(previous) = previous {
            require(
                previous.identity == self.identity && previous.end.is_none(),
                "terminal observation cannot revive",
            )?;
            require(
                self.receipt_sequence > previous.receipt_sequence,
                "receipt sequence regressed",
            )?;
            require(
                self.accepted_actions >= previous.accepted_actions
                    && self.started_actions >= previous.started_actions
                    && self.submitted_actions >= previous.submitted_actions
                    && self.submitted_frames >= previous.submitted_frames,
                "submission evidence regressed",
            )?;
            require(
                previous
                    .backend
                    .as_ref()
                    .is_none_or(|backend| self.backend.as_ref() == Some(backend)),
                "backend changed or disappeared",
            )?;
            require(
                !previous.writer_quiescent || self.writer_quiescent,
                "quiescent writer restarted",
            )?;
            require(
                !previous.writer_quiescent
                    || (self.started_actions == previous.started_actions
                        && self.submitted_actions == previous.submitted_actions
                        && self.submitted_frames == previous.submitted_frames
                        && self.partial_frame == previous.partial_frame
                        && self.last_partial_channels == previous.last_partial_channels
                        && self.last_uncertain_channels == previous.last_uncertain_channels),
                "native facts grew after writer quiescence",
            )?;
            // A partial/uncertain native write is a final fault boundary, never a
            // reason to retry the same physical frame on another observation.
            require(
                previous.partial_frame.is_none()
                    || (self.partial_frame == previous.partial_frame
                        && self.last_partial_channels == previous.last_partial_channels
                        && self.last_uncertain_channels == previous.last_uncertain_channels),
                "partial native facts changed",
            )?;
        }
        if let Some(end) = &self.end {
            match end {
                BodyEnd::Submitted => {
                    require(
                        self.writer_quiescent
                            && !partial
                            && self.accepted_actions == command.actions.len() as u32
                            && self.submitted_actions == self.accepted_actions
                            && self.backend.is_some(),
                        "submitted without complete quiescent proof",
                    )?;
                }
                BodyEnd::Cancelled { .. } | BodyEnd::Failed { .. } => {
                    require(self.writer_quiescent, "terminal stop lacks quiescence")?
                }
                BodyEnd::Rejected { .. } => require(
                    self.writer_quiescent
                        && self.started_actions == 0
                        && self.submitted_frames == 0
                        && !partial,
                    "rejection after native output",
                )?,
                BodyEnd::Unknown { .. } => {}
            }
        }
        Ok(())
    }
}

/// Signed requests include a fresh request ID. Replies echo it, so an old
/// signed control-state response cannot authorize opening a replacement lease.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyRequest {
    pub version: u32,
    pub request_id: Uuid,
    pub device_id: String,
    pub issued_at_unix_ms: u64,
    pub operation: BodyOperation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BodyOperation {
    Inspect,
    Open {
        device_boot_id: Uuid,
        expected_generation: u64,
        server_instance_id: Uuid,
    },
    Submit {
        command: BodyCommand,
    },
    Observe {
        identity: BodyCommandIdentity,
        after_receipt_sequence: u64,
    },
    Cancel {
        /// Original immutable intent also fences a Submit still in preparation.
        command: BodyCommand,
        reason: String,
    },
    Close {
        lease: BodyControlLease,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyControlState {
    pub device_boot_id: Uuid,
    pub generation: u64,
    pub lease: Option<BodyControlLease>,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyReply {
    pub version: u32,
    pub request_id: Uuid,
    pub device_id: String,
    pub evidence: BodyEvidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BodyEvidence {
    Control {
        state: BodyControlState,
    },
    Progress {
        progress: BodyProgress,
    },
    /// The identity is below the admission waterline but its bounded receipt was
    /// retired. This is never permission to submit again with another identity.
    Unavailable {
        identity: BodyCommandIdentity,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> BodyCommand {
        BodyCommand {
            identity: BodyCommandIdentity {
                lease: BodyControlLease {
                    device_boot_id: Uuid::new_v4(),
                    server_instance_id: Uuid::new_v4(),
                    generation: 1,
                },
                sequence: 1,
            },
            plan_id: Uuid::new_v4(),
            source_command_id: "plan:speech_start".into(),
            owner_id: Uuid::new_v4(),
            scope: BodyScope::Nonvoice {
                scope_id: Uuid::new_v4(),
            },
            priority: BodyPriority::Normal,
            interruptible: true,
            actions: vec![BodyAction {
                duration_ms: 400,
                intensity: 0.5,
                motion: BodyMotion::SmallNod,
            }],
        }
    }

    fn receipt(command: &BodyCommand) -> BodyProgress {
        BodyProgress {
            identity: command.identity,
            receipt_sequence: 1,
            accepted_actions: 1,
            started_actions: 1,
            submitted_actions: 1,
            submitted_frames: 10,
            partial_frame: None,
            last_partial_channels: vec![],
            last_uncertain_channels: vec![],
            backend: Some(BodyBackend {
                instance_id: Uuid::new_v4(),
                boundary: BodySubmissionBoundary::Simulated,
                channels: 3,
            }),
            writer_quiescent: true,
            position_confirmed_actions: None,
            end: Some(BodyEnd::Submitted),
        }
    }

    #[test]
    fn legacy_animation_without_content_identity_is_rejected() {
        let mut legacy = serde_json::to_value(command()).unwrap();
        legacy["actions"][0]["motion"] = serde_json::json!({
            "kind": "animation", "file": "reviewed.anim", "fps": 30, "loops": 1
        });
        let rejected = match serde_json::from_value::<BodyCommand>(legacy) {
            Err(_) => true,
            Ok(command) => command.validate().is_err(),
        };
        assert!(
            rejected,
            "a signed animation without an approved content identity was accepted"
        );
    }

    #[test]
    fn approved_animation_identity_binds_exact_bytes_and_rejects_invalid_catalogs() {
        let asset = BodyAnimationAsset {
            file: "reviewed.anim".into(),
            version: 1,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
            bytes: 3,
        };
        assert!(asset.verify_bytes(b"abc").is_ok());
        assert!(asset.verify_bytes(b"abd").is_err());
        assert!(asset.verify_bytes(b"abc\n").is_err());
        for (file, version, sha256, bytes) in [
            ("../reviewed.anim", 1, asset.sha256.clone(), 3),
            ("reviewed", 1, asset.sha256.clone(), 3),
            (".anim", 1, asset.sha256.clone(), 3),
            ("reviewed.anim", 0, asset.sha256.clone(), 3),
            ("reviewed.anim", 1, asset.sha256.to_uppercase(), 3),
            ("reviewed.anim", 1, "a".repeat(63), 3),
            ("reviewed.anim", 1, asset.sha256.clone(), 0),
            ("reviewed.anim", 1, asset.sha256.clone(), 65_537),
        ] {
            assert!(
                BodyAnimationAsset {
                    file: file.into(),
                    version,
                    sha256,
                    bytes
                }
                .validate()
                .is_err()
            );
        }
        assert!(validate_animation_catalog(&[]).is_ok());
        assert!(validate_animation_catalog(std::slice::from_ref(&asset)).is_ok());
        assert!(validate_animation_catalog(&[asset.clone(), asset.clone()]).is_err());
        let too_many: Vec<_> = (0..65)
            .map(|n| BodyAnimationAsset {
                file: format!("asset-{n}.anim"),
                ..asset.clone()
            })
            .collect();
        assert!(validate_animation_catalog(&too_many).is_err());
        let mut unknown = serde_json::to_value(&asset).unwrap();
        unknown["approved"] = serde_json::json!(true);
        assert!(serde_json::from_value::<BodyAnimationAsset>(unknown).is_err());
    }

    #[test]
    fn rejects_nonfinite_unsafe_and_unbounded_intent_before_any_backend() {
        let mut c = command();
        assert!(c.validate().is_ok());
        c.actions[0].intensity = f32::NAN;
        assert!(c.validate().is_err());
        c.actions[0].intensity = 0.5;
        c.actions[0].motion = BodyMotion::Animation {
            asset: BodyAnimationAsset {
                file: "../secret.anim".into(),
                version: 1,
                sha256: "0".repeat(64),
                bytes: 1,
            },
            fps: 30,
            loops: 1,
        };
        assert!(c.validate().is_err());
        c.actions[0].motion = BodyMotion::SmallNod;
        c.actions = vec![c.actions[0].clone(); 13];
        assert!(c.validate().is_err());
        c.actions.truncate(1);
        c.scope = BodyScope::Voice {
            scope: VoiceScope {
                device_boot_id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                generation_epoch: 1,
            },
        };
        assert!(c.validate().is_err());
    }

    #[test]
    fn submission_does_not_supply_position_or_cancel_quiescence() {
        let c = command();
        let mut p = receipt(&c);
        assert!(p.validate(None, &c).is_ok());
        p.started_actions = 0;
        p.submitted_actions = 0;
        assert!(p.validate(None, &c).is_err());
        p = receipt(&c);
        p.position_confirmed_actions = Some(1);
        assert!(p.validate(None, &c).is_err());
        p.position_confirmed_actions = None;
        p.writer_quiescent = false;
        p.end = Some(BodyEnd::Cancelled {
            reason: "timeout".into(),
        });
        assert!(p.validate(None, &c).is_err());
        p.end = Some(BodyEnd::Unknown {
            reason: "write pending".into(),
        });
        assert!(p.validate(None, &c).is_ok());
    }

    #[test]
    fn preserves_partial_native_facts_and_rejects_forged_monotonic_receipts() {
        let c = command();
        let mut p = receipt(&c);
        p.submitted_actions = 0;
        p.partial_frame = Some(10);
        p.last_partial_channels = vec![0];
        p.last_uncertain_channels = vec![1];
        p.end = None;
        assert!(p.validate(None, &c).is_ok());
        let mut next = p.clone();
        next.receipt_sequence += 1;
        next.end = Some(BodyEnd::Failed {
            reason: "partial I2C write".into(),
        });
        assert!(next.validate(Some(&p), &c).is_ok());
        let mut after_quiescence = p.clone();
        after_quiescence.partial_frame = None;
        after_quiescence.last_partial_channels.clear();
        after_quiescence.last_uncertain_channels.clear();
        let mut impossible = after_quiescence.clone();
        impossible.receipt_sequence += 1;
        impossible.submitted_frames += 1;
        assert!(impossible.validate(Some(&after_quiescence), &c).is_err());
        next.last_uncertain_channels.clear();
        assert!(next.validate(Some(&p), &c).is_err());
        next = p.clone();
        next.receipt_sequence += 1;
        next.identity.sequence += 1;
        assert!(next.validate(Some(&p), &c).is_err());
    }
}
