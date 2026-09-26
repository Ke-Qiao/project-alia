//! Frozen inputs to Companion's deterministic invitation decision.
//!
//! Deserializing these types is useful for offline replay, but grants no
//! authority. Production constructs them from canonical SQL and runtime facts
//! and revalidates those facts when admitting any output.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SCHEMA_VERSION: u16 = 1;
pub const GENERATOR_VERSION: &str = "alia.companion.invitation.v1";
pub const DECISION_VERSION: &str = "alia.companion.decision.v1";
pub const MAX_APPRAISAL_REFERENCES: usize = 8;
pub const MAX_CANDIDATE_BYTES: usize = 16 * 1024;
pub const MAX_DECISION_INPUT_BYTES: usize = 24 * 1024;
pub const MAX_DECISION_RECORD_BYTES: usize = 4 * 1024;
pub const MAX_OPPORTUNITY_SECONDS: i64 = 10;
pub const MAX_CAPTURE_AGE_SECONDS: i64 = 5;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompanionMode {
    Disabled,
    #[default]
    Shadow,
    Enabled,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompanionPolicy {
    pub mode: CompanionMode,
    pub quiet: bool,
    pub paused: bool,
    /// Admissions per UTC date, including outcomes that remain unknown.
    pub daily_budget: u16,
    pub cooldown_seconds: u32,
    pub candidate_ttl_seconds: u32,
    pub require_response_before_next: bool,
}

impl Default for CompanionPolicy {
    fn default() -> Self {
        Self {
            mode: CompanionMode::Shadow,
            quiet: false,
            paused: false,
            daily_budget: 3,
            cooldown_seconds: 900,
            candidate_ttl_seconds: 60,
            require_response_before_next: true,
        }
    }
}

impl CompanionPolicy {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.daily_budget <= 24
                && (30..=86_400).contains(&self.cooldown_seconds)
                && (1..=300).contains(&self.candidate_ttl_seconds),
            "companion policy exceeds its bounds"
        );
        Ok(())
    }

    pub fn digest(&self) -> anyhow::Result<String> {
        self.validate()?;
        super::decision::canonical_digest(self)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolicySnapshot {
    pub revision: i64,
    pub policy: CompanionPolicy,
}

impl PolicySnapshot {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.revision > 0, "invalid companion policy revision");
        self.policy.validate()
    }

    pub fn digest(&self) -> anyhow::Result<String> {
        self.validate()?;
        super::decision::canonical_digest(self)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct AppraisalReference {
    pub appraisal_id: Uuid,
    pub revision: i64,
    pub result_digest: String,
}

impl AppraisalReference {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.appraisal_id.is_nil() && self.revision > 0,
            "invalid companion appraisal reference"
        );
        validate_digest(&self.result_digest)
    }
}

/// A bounded canonical source header. No message text, inferred emotion,
/// free-form topic, or private reason is copied into the decision record.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CanonicalObservation {
    pub owner_id: Uuid,
    pub context_id: Uuid,
    pub request_id: Uuid,
    pub source_event_id: Uuid,
    pub conversation_id: Uuid,
    pub channel_id: String,
    pub source_received_at: DateTime<Utc>,
    /// Original canonical Appraisal time, never the worker's scan time.
    pub effective_at: DateTime<Utc>,
    pub privacy_epoch: i64,
    pub relationship_epoch: i64,
    pub context_policy_version: i64,
    pub appraisals: Vec<AppraisalReference>,
    pub valid_until: DateTime<Utc>,
}

impl CanonicalObservation {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            [
                self.owner_id,
                self.context_id,
                self.request_id,
                self.source_event_id,
                self.conversation_id,
            ]
            .iter()
            .all(|id| !id.is_nil())
                && self.privacy_epoch >= 0
                && self.relationship_epoch >= 0
                && self.context_policy_version > 0,
            "invalid companion source identity"
        );
        anyhow::ensure!(
            !self.channel_id.is_empty()
                && self.channel_id.len() <= 256
                && !self.channel_id.chars().any(char::is_control),
            "invalid companion channel identity"
        );
        anyhow::ensure!(
            (1..=MAX_APPRAISAL_REFERENCES).contains(&self.appraisals.len()),
            "companion appraisal reference bound exceeded"
        );
        let mut ids = std::collections::BTreeSet::new();
        for reference in &self.appraisals {
            reference.validate()?;
            anyhow::ensure!(
                ids.insert(reference.appraisal_id),
                "duplicate companion appraisal reference"
            );
        }
        for value in [self.source_received_at, self.effective_at, self.valid_until] {
            validate_time(value)?;
        }
        anyhow::ensure!(
            self.source_received_at <= self.effective_at
                && self.effective_at < self.valid_until
                && self.valid_until - self.effective_at <= Duration::days(366),
            "invalid companion source lifetime"
        );
        bounded_json(self, MAX_CANDIDATE_BYTES)
    }

    pub fn digest(&self) -> anyhow::Result<String> {
        self.validate()?;
        let mut canonical = self.clone();
        canonical.appraisals.sort();
        super::decision::canonical_digest(&canonical)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    FollowUpInvitation,
}

impl CandidateKind {
    pub fn text(self) -> &'static str {
        match self {
            Self::FollowUpInvitation => "如果你愿意，我们可以再聊一会儿。",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub id: Uuid,
    pub dedup_key: String,
    pub kind: CandidateKind,
    pub generator_version: String,
    pub observation: CanonicalObservation,
    pub policy: PolicySnapshot,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl Candidate {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.observation.validate()?;
        self.policy.validate()?;
        validate_time(self.created_at)?;
        validate_time(self.expires_at)?;
        anyhow::ensure!(
            self.generator_version == GENERATOR_VERSION,
            "unsupported companion generator version"
        );
        anyhow::ensure!(
            self.created_at == self.observation.effective_at,
            "companion candidate changed original effective time"
        );
        let expected_expiry = self
            .created_at
            .checked_add_signed(Duration::seconds(i64::from(
                self.policy.policy.candidate_ttl_seconds,
            )))
            .ok_or_else(|| anyhow::anyhow!("companion candidate expiry overflow"))?
            .min(self.observation.valid_until);
        anyhow::ensure!(
            self.expires_at == expected_expiry && self.expires_at > self.created_at,
            "companion candidate changed original expiry"
        );
        anyhow::ensure!(
            self.observation
                .appraisals
                .windows(2)
                .all(|pair| pair[0] < pair[1]),
            "companion appraisal references are not canonical"
        );
        let (id, dedup_key) = super::decision::candidate_identity(&self.observation, self.kind)?;
        anyhow::ensure!(
            self.id == id && self.dedup_key == dedup_key,
            "companion candidate identity differs from its source"
        );
        bounded_json(self, MAX_CANDIDATE_BYTES)
    }

    pub fn digest(&self) -> anyhow::Result<String> {
        self.validate()?;
        super::decision::canonical_digest(self)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OpportunitySource {
    Unavailable,
    Simulation,
    VerifiedLive {
        server_instance_id: Uuid,
        device_boot_id: Uuid,
        session_id: Uuid,
        generation_epoch: u64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    Unknown,
    Idle,
    UserSpeaking,
    InteractionBusy,
    OutputBusy,
    OutputHeld,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OwnerAssurance {
    Unknown,
    BoundOwner,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputCapability {
    Unavailable,
    FollowUpVoice,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OpportunitySnapshot {
    pub source: OpportunitySource,
    pub observed_at: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
    pub activity: ActivityState,
    pub owner_assurance: OwnerAssurance,
    pub capability: OutputCapability,
    pub capture_revision: Option<u64>,
    pub last_capture_at: Option<DateTime<Utc>>,
}

impl OpportunitySnapshot {
    /// Missing runtime evidence is unknown, never simulated presence or idleness.
    pub fn unavailable(as_of: DateTime<Utc>) -> Self {
        Self {
            source: OpportunitySource::Unavailable,
            observed_at: as_of,
            valid_until: as_of,
            activity: ActivityState::Unknown,
            owner_assurance: OwnerAssurance::Unknown,
            capability: OutputCapability::Unavailable,
            capture_revision: None,
            last_capture_at: None,
        }
    }

    /// Explicit offline fixture only. This can never produce `Allowed`.
    pub fn simulation(as_of: DateTime<Utc>) -> Self {
        Self {
            source: OpportunitySource::Simulation,
            observed_at: as_of,
            valid_until: as_of + Duration::seconds(MAX_OPPORTUNITY_SECONDS),
            activity: ActivityState::Idle,
            owner_assurance: OwnerAssurance::BoundOwner,
            capability: OutputCapability::FollowUpVoice,
            capture_revision: None,
            last_capture_at: None,
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_time(self.observed_at)?;
        validate_time(self.valid_until)?;
        anyhow::ensure!(
            self.valid_until >= self.observed_at
                && self.valid_until - self.observed_at
                    <= Duration::seconds(MAX_OPPORTUNITY_SECONDS),
            "invalid companion opportunity lifetime"
        );
        match &self.source {
            OpportunitySource::Unavailable => anyhow::ensure!(
                self.activity == ActivityState::Unknown
                    && self.owner_assurance == OwnerAssurance::Unknown
                    && self.capability == OutputCapability::Unavailable
                    && self.capture_revision.is_none()
                    && self.last_capture_at.is_none(),
                "unavailable opportunity cannot assert live facts"
            ),
            OpportunitySource::Simulation => (),
            OpportunitySource::VerifiedLive {
                server_instance_id,
                device_boot_id,
                session_id,
                generation_epoch,
            } => anyhow::ensure!(
                !server_instance_id.is_nil()
                    && !device_boot_id.is_nil()
                    && !session_id.is_nil()
                    && *generation_epoch > 0,
                "invalid live companion opportunity identity"
            ),
        }
        if let Some(at) = self.last_capture_at {
            validate_time(at)?;
            anyhow::ensure!(
                at <= self.observed_at,
                "capture is newer than its observation"
            );
        }
        anyhow::ensure!(
            self.capture_revision.is_some() == self.last_capture_at.is_some(),
            "invalid companion capture watermark"
        );
        Ok(())
    }

    pub fn digest(&self) -> anyhow::Result<String> {
        self.validate()?;
        super::decision::canonical_digest(self)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BudgetSnapshot {
    pub revision: u64,
    pub as_of: DateTime<Utc>,
    pub utc_date: NaiveDate,
    pub admitted_today: u16,
    pub last_admitted_at: Option<DateTime<Utc>>,
    pub deduplicated: bool,
    /// Canonical response tracking only; absence never means user dislike.
    pub awaiting_response: bool,
}

impl BudgetSnapshot {
    pub fn empty(as_of: DateTime<Utc>) -> Self {
        Self {
            revision: 0,
            as_of,
            utc_date: as_of.date_naive(),
            admitted_today: 0,
            last_admitted_at: None,
            deduplicated: false,
            awaiting_response: false,
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_time(self.as_of)?;
        anyhow::ensure!(
            self.utc_date == self.as_of.date_naive() && self.admitted_today <= 24,
            "invalid companion UTC budget window"
        );
        if let Some(at) = self.last_admitted_at {
            validate_time(at)?;
            anyhow::ensure!(
                at <= self.as_of,
                "companion budget includes a future admission"
            );
        }
        anyhow::ensure!(
            self.admitted_today == 0 || self.last_admitted_at.is_some(),
            "companion budget omitted its latest admission"
        );
        Ok(())
    }

    pub fn digest(&self) -> anyhow::Result<String> {
        self.validate()?;
        super::decision::canonical_digest(self)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DecisionInput {
    pub candidate: Candidate,
    pub current_policy: PolicySnapshot,
    pub as_of: DateTime<Utc>,
    pub opportunity: OpportunitySnapshot,
    pub budget: BudgetSnapshot,
}

impl DecisionInput {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.candidate.validate()?;
        self.current_policy.validate()?;
        self.opportunity.validate()?;
        self.budget.validate()?;
        validate_time(self.as_of)?;
        anyhow::ensure!(
            self.as_of >= self.candidate.created_at
                && self.as_of >= self.opportunity.observed_at
                && self.budget.as_of == self.as_of,
            "companion decision has inconsistent frozen times"
        );
        bounded_json(self, MAX_DECISION_INPUT_BYTES)
    }

    pub fn digest(&self) -> anyhow::Result<String> {
        self.validate()?;
        super::decision::canonical_digest(self)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    Suppressed,
    ShadowWouldAllow,
    /// Pure evaluation only; callers still need canonical transactional admission.
    Allowed,
}

/// Declaration order is the versioned, stable ordering in a decision record.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DecisionReason {
    PolicyDisabled,
    Quiet,
    Paused,
    PolicyChanged,
    CandidateExpired,
    OpportunityExpired,
    OpportunityUnavailable,
    ActivityUnknown,
    UserSpeaking,
    InteractionBusy,
    OutputBusy,
    OutputHeld,
    OwnerUnverified,
    CapabilityUnavailable,
    CaptureUnavailable,
    CaptureStale,
    DailyBudgetExhausted,
    Cooldown,
    Duplicate,
    AwaitingResponse,
    ShadowMode,
    SimulatedOpportunity,
    ShadowOrigin,
}

impl DecisionReason {
    pub fn suppresses(self) -> bool {
        !matches!(
            self,
            Self::ShadowMode | Self::SimulatedOpportunity | Self::ShadowOrigin
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DecisionRecord {
    pub schema_version: u16,
    pub evaluator_version: String,
    pub candidate_id: Uuid,
    pub as_of: DateTime<Utc>,
    pub outcome: DecisionOutcome,
    pub reasons: Vec<DecisionReason>,
    pub valid_until: DateTime<Utc>,
    pub input_digest: String,
    pub result_digest: String,
}

impl DecisionRecord {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.schema_version == SCHEMA_VERSION
                && self.evaluator_version == DECISION_VERSION
                && !self.candidate_id.is_nil()
                && self.reasons.len() <= 23
                && self.reasons.windows(2).all(|pair| pair[0] < pair[1]),
            "invalid companion decision record"
        );
        validate_time(self.as_of)?;
        validate_time(self.valid_until)?;
        validate_digest(&self.input_digest)?;
        validate_digest(&self.result_digest)?;
        let expected_outcome = if self.reasons.iter().any(|reason| reason.suppresses()) {
            DecisionOutcome::Suppressed
        } else if !self.reasons.is_empty() {
            DecisionOutcome::ShadowWouldAllow
        } else {
            DecisionOutcome::Allowed
        };
        anyhow::ensure!(
            self.outcome == expected_outcome,
            "decision outcome contradicts its reasons"
        );
        anyhow::ensure!(
            self.outcome == DecisionOutcome::Suppressed || self.as_of < self.valid_until,
            "non-suppressed companion decision has expired"
        );
        anyhow::ensure!(
            self.result_digest == super::decision::result_digest(self)?,
            "companion decision result digest mismatch"
        );
        bounded_json(self, MAX_DECISION_RECORD_BYTES)
    }

    pub fn digest(&self) -> anyhow::Result<String> {
        self.validate()?;
        Ok(self.result_digest.clone())
    }
}

pub(super) fn validate_time(value: DateTime<Utc>) -> anyhow::Result<()> {
    anyhow::ensure!(
        (0..=253_402_300_799).contains(&value.timestamp()),
        "companion timestamp must be a bounded UTC value"
    );
    Ok(())
}

pub(super) fn validate_digest(value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid companion SHA-256 digest"
    );
    Ok(())
}

pub(super) fn bounded_json(value: &impl Serialize, maximum: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        serde_json::to_vec(value)?.len() <= maximum,
        "companion JSON byte bound exceeded"
    );
    Ok(())
}
