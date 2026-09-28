use alia_core::delivery::VoiceScope;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::{decision::canonical_digest, types::*};
use crate::modules::{
    delivery::types::ResponseRecord, interaction::initiative::VoiceOpportunity,
    memory::context::ContextBinding,
};

pub(crate) const INVITATION_KIND: &str = "companion_invitation";
pub(crate) const INVITATION_OUTPUT_KEY: &str = "companion_invitation";
pub(crate) const INVITATION_MEDIA_KEY: &str = "companion_invitation";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct InvitationContent {
    pub schema_version: u16,
    pub invitation_kind: CandidateKind,
    pub generator_version: String,
    pub text: String,
    pub voice_scope: VoiceScope,
    pub candidate_id: Uuid,
    pub admission_id: Uuid,
    pub source_event_id: Uuid,
}
impl InvitationContent {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.schema_version == 1
                && self.invitation_kind == CandidateKind::FollowUpInvitation
                && self.generator_version == GENERATOR_VERSION
                && self.text == CandidateKind::FollowUpInvitation.text()
                && self.voice_scope.is_valid()
                && !self.candidate_id.is_nil()
                && !self.admission_id.is_nil()
                && !self.source_event_id.is_nil(),
            "invalid fixed invitation content"
        );
        Ok(())
    }
    pub fn digest(&self) -> anyhow::Result<String> {
        self.validate()?;
        canonical_digest(self)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenVoiceOpportunity {
    pub id: Uuid,
    pub owner: Uuid,
    pub channel_id: String,
    pub scope: VoiceScope,
    pub speech_revision: u64,
    pub through_sequence: u64,
    pub observed_at: DateTime<Utc>,
    pub last_capture_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}
impl FrozenVoiceOpportunity {
    pub(super) fn freeze(value: &VoiceOpportunity) -> Self {
        Self {
            id: value.id,
            owner: value.owner,
            channel_id: value.channel_id.clone(),
            scope: value.scope,
            speech_revision: value.speech_revision,
            through_sequence: value.through_sequence,
            observed_at: value.observed_at,
            last_capture_at: value.last_capture_at,
            expires_at: value.expires_at,
        }
    }
    pub fn snapshot(&self, instance: Uuid) -> OpportunitySnapshot {
        OpportunitySnapshot {
            source: OpportunitySource::VerifiedLive {
                server_instance_id: instance,
                device_boot_id: self.scope.device_boot_id,
                session_id: self.scope.session_id,
                generation_epoch: self.scope.generation_epoch,
            },
            observed_at: self.observed_at,
            valid_until: self.expires_at,
            activity: ActivityState::Idle,
            owner_assurance: OwnerAssurance::BoundOwner,
            capability: OutputCapability::FollowUpVoice,
            capture_revision: Some(self.speech_revision),
            last_capture_at: Some(self.last_capture_at),
        }
    }
}

/// Private canonical grant. SQL erasure removes the entire value and its digest.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LiveAdmissionGrant {
    pub schema_version: u16,
    pub admission_id: Uuid,
    pub candidate: Candidate,
    pub context_id: Uuid,
    pub initiative_request_id: Uuid,
    pub server_instance_id: Uuid,
    pub opportunity: FrozenVoiceOpportunity,
    pub expected_scope: VoiceScope,
    pub content: InvitationContent,
    pub admit_before: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
    pub intent_digest: String,
    pub input: DecisionInput,
    pub decision: DecisionRecord,
}
impl LiveAdmissionGrant {
    pub fn digest(&self) -> anyhow::Result<String> {
        canonical_digest(self)
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        self.content.validate()?;
        self.candidate.validate()?;
        anyhow::ensure!(
            self.schema_version == 1
                && !self.context_id.is_nil()
                && !self.initiative_request_id.is_nil()
                && !self.admission_id.is_nil()
                && !self.server_instance_id.is_nil()
                && !self.opportunity.id.is_nil()
                && self.input.candidate == self.candidate
                && self.input.opportunity == self.opportunity.snapshot(self.server_instance_id)
                && self.opportunity.owner == self.candidate.observation.owner_id
                && self.opportunity.channel_id == self.candidate.observation.channel_id
                && self.expected_scope.device_boot_id == self.opportunity.scope.device_boot_id
                && self.expected_scope.session_id == self.opportunity.scope.session_id
                && Some(self.expected_scope.generation_epoch)
                    == self.opportunity.scope.generation_epoch.checked_add(1)
                && self.content.voice_scope == self.expected_scope
                && self.content.candidate_id == self.candidate.id
                && self.content.admission_id == self.admission_id
                && self.content.source_event_id == self.candidate.observation.source_event_id
                && self.input.as_of < self.admit_before
                && self.admit_before <= self.decision.valid_until
                && self.admit_before
                    <= self.opportunity.last_capture_at + chrono::Duration::seconds(2)
                && self.valid_until == self.candidate.expires_at
                && self.decision.outcome == DecisionOutcome::Allowed
                && super::super::decision::replay(&self.input, &self.decision)?,
            "invalid live admission grant"
        );
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PendingInvitation {
    pub candidate_id: Uuid,
    pub owner_id: Uuid,
    pub channel_id: String,
    pub expires_at: DateTime<Utc>,
}

/// Same immutable in-memory intent is retained by one managed attempt owner.
#[derive(Clone, Debug)]
pub(crate) struct LiveAdmissionIntent {
    pub(super) candidate: Candidate,
    pub(super) admission_id: Uuid,
    pub(super) context_id: Uuid,
    pub(super) initiative_request_id: Uuid,
    pub(super) server_instance_id: Uuid,
    pub(super) opportunity: VoiceOpportunity,
    pub(super) expected_scope: VoiceScope,
    pub(super) admit_before: DateTime<Utc>,
    pub(super) valid_until: DateTime<Utc>,
    pub(super) digest: String,
}
impl LiveAdmissionIntent {
    pub fn candidate_id(&self) -> Uuid {
        self.candidate.id
    }
    pub fn admission_id(&self) -> Uuid {
        self.admission_id
    }
    pub fn admit_before(&self) -> DateTime<Utc> {
        self.admit_before
    }
}

/// No Clone/Deserialize and no lookup constructor. Owned by exactly one reserve.
#[derive(Debug)]
pub(crate) struct LiveAdmissionPermit {
    admission_id: Uuid,
    candidate_id: Uuid,
    initiative_id: Uuid,
    server_instance_id: Uuid,
    opportunity: VoiceOpportunity,
    context: ContextBinding,
    expected_scope: VoiceScope,
    deadline: DateTime<Utc>,
    admit_before: DateTime<Utc>,
}
impl LiveAdmissionPermit {
    pub(super) fn committed(intent: &LiveAdmissionIntent, context: ContextBinding) -> Self {
        Self {
            admission_id: intent.admission_id,
            candidate_id: intent.candidate.id,
            initiative_id: intent.initiative_request_id,
            server_instance_id: intent.server_instance_id,
            opportunity: intent.opportunity.clone(),
            context,
            expected_scope: intent.expected_scope,
            deadline: intent.valid_until,
            admit_before: intent.admit_before,
        }
    }
    pub fn admission_id(&self) -> Uuid {
        self.admission_id
    }
    pub fn candidate_id(&self) -> Uuid {
        self.candidate_id
    }
    pub fn initiative_id(&self) -> Uuid {
        self.initiative_id
    }
    pub fn server_instance_id(&self) -> Uuid {
        self.server_instance_id
    }
    pub fn context(&self) -> &ContextBinding {
        &self.context
    }
    pub fn opportunity(&self) -> &VoiceOpportunity {
        &self.opportunity
    }
    pub fn expected_scope(&self) -> VoiceScope {
        self.expected_scope
    }
    pub fn deadline(&self) -> DateTime<Utc> {
        self.deadline
    }
    pub fn admit_before(&self) -> DateTime<Utc> {
        self.admit_before
    }
    #[cfg(test)]
    pub(crate) fn fixture(
        opportunity: VoiceOpportunity,
        context: ContextBinding,
        deadline: DateTime<Utc>,
    ) -> Self {
        Self {
            admission_id: Uuid::new_v4(),
            candidate_id: Uuid::new_v4(),
            initiative_id: context.request_id,
            server_instance_id: Uuid::new_v4(),
            expected_scope: VoiceScope {
                generation_epoch: opportunity.scope.generation_epoch.checked_add(1).unwrap(),
                ..opportunity.scope
            },
            opportunity,
            context,
            deadline,
            admit_before: deadline,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AdmissionState {
    Active,
    Retired,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdmissionReceipt {
    pub admission_id: Uuid,
    pub candidate_id: Uuid,
    pub initiative_request_id: Uuid,
    pub state: AdmissionState,
    pub charged_at: DateTime<Utc>,
    pub budget_date: NaiveDate,
}
#[derive(Debug)]
pub(crate) enum AdmissionResult {
    Fresh {
        permit: LiveAdmissionPermit,
        context: ContextBinding,
        response: ResponseRecord,
    },
    Existing(AdmissionReceipt),
    Deferred(Vec<DecisionReason>),
    Retired,
}
#[derive(Debug, thiserror::Error)]
pub(crate) enum LiveError {
    #[error("invalid live Companion input")]
    InvalidInput,
    #[error("live Companion intent conflict")]
    Conflict,
    #[error("live Companion access denied")]
    Forbidden,
    #[error("live Companion unavailable: {0}")]
    Unavailable(String),
    #[error("live Companion commit acknowledgement unknown: {admission_id}")]
    CommitUnknown {
        candidate_id: Uuid,
        admission_id: Uuid,
    },
}
impl From<sea_orm::DbErr> for LiveError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Unavailable(e.to_string())
    }
}
impl From<anyhow::Error> for LiveError {
    fn from(e: anyhow::Error) -> Self {
        Self::Unavailable(e.to_string())
    }
}
impl From<serde_json::Error> for LiveError {
    fn from(e: serde_json::Error) -> Self {
        Self::Unavailable(e.to_string())
    }
}
impl From<super::super::repository::RepoError> for LiveError {
    fn from(e: super::super::repository::RepoError) -> Self {
        use super::super::repository::RepoError;
        match e {
            RepoError::InvalidInput(_) => Self::InvalidInput,
            RepoError::Conflict => Self::Conflict,
            RepoError::Forbidden => Self::Forbidden,
            other => Self::Unavailable(other.to_string()),
        }
    }
}
pub(crate) type LiveResult<T> = Result<T, LiveError>;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RetirementReason {
    Expired,
    SourceRevoked,
    PolicyChanged,
    InstanceEnded,
    AttemptFailed,
    Shutdown,
}
impl RetirementReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::SourceRevoked => "source_revoked",
            Self::PolicyChanged => "policy_changed",
            Self::InstanceEnded => "instance_ended",
            Self::AttemptFailed => "attempt_failed",
            Self::Shutdown => "shutdown",
        }
    }
}
