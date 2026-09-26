//! Pure, versioned decisions. No clock, database, model, channel, or device is
//! consulted here, and `Allowed` is never an output capability.

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::types::*;

/// Shares the canonical JSON digest used by the persisted Context contracts.
pub fn canonical_digest(value: &impl Serialize) -> anyhow::Result<String> {
    crate::modules::memory::context::evidence_digest(&serde_json::to_value(value)?)
}

pub fn generate_candidate(
    observation: &CanonicalObservation,
    policy: &PolicySnapshot,
    created_at: DateTime<Utc>,
) -> anyhow::Result<Candidate> {
    observation.validate()?;
    policy.validate()?;
    anyhow::ensure!(
        created_at == observation.effective_at,
        "candidate creation must retain the canonical appraisal time"
    );
    let mut observation = observation.clone();
    observation.appraisals.sort();
    let kind = CandidateKind::FollowUpInvitation;
    let (id, dedup_key) = candidate_identity(&observation, kind)?;
    let expires_at = created_at
        .checked_add_signed(Duration::seconds(i64::from(
            policy.policy.candidate_ttl_seconds,
        )))
        .ok_or_else(|| anyhow::anyhow!("companion candidate expiry overflow"))?
        .min(observation.valid_until);
    let candidate = Candidate {
        id,
        dedup_key,
        kind,
        generator_version: GENERATOR_VERSION.into(),
        observation,
        policy: policy.clone(),
        created_at,
        expires_at,
    };
    candidate.validate()?;
    Ok(candidate)
}

pub(super) fn candidate_identity(
    observation: &CanonicalObservation,
    kind: CandidateKind,
) -> anyhow::Result<(Uuid, String)> {
    // Neither a new operation, appraisal rebuild, nor policy revision creates
    // another invitation identity for the same actual user source.
    let identity = serde_json::to_vec(&(
        GENERATOR_VERSION,
        observation.owner_id,
        observation.source_event_id,
        kind,
    ))?;
    let hash: [u8; 32] = Sha256::digest(&identity).into();
    let mut bytes: [u8; 16] = hash[..16].try_into().expect("SHA-256 prefix is 16 bytes");
    // RFC 9562 custom UUID, with an explicit algorithm/version in the input.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok((
        Uuid::from_bytes(bytes),
        format!("{:x}", Sha256::digest(&identity)),
    ))
}

pub fn decide(input: &DecisionInput) -> anyhow::Result<DecisionRecord> {
    input.validate()?;
    let policy = &input.current_policy.policy;
    let opportunity = &input.opportunity;
    let budget = &input.budget;
    let mut reasons = Vec::new();
    if policy.mode == CompanionMode::Disabled {
        reasons.push(DecisionReason::PolicyDisabled);
    }
    if policy.quiet {
        reasons.push(DecisionReason::Quiet);
    }
    if policy.paused {
        reasons.push(DecisionReason::Paused);
    }
    if input.current_policy != input.candidate.policy {
        reasons.push(DecisionReason::PolicyChanged);
    }
    if input.as_of >= input.candidate.expires_at {
        reasons.push(DecisionReason::CandidateExpired);
    }
    if input.as_of >= opportunity.valid_until {
        reasons.push(DecisionReason::OpportunityExpired);
    }
    match &opportunity.source {
        OpportunitySource::Unavailable => reasons.push(DecisionReason::OpportunityUnavailable),
        OpportunitySource::Simulation => reasons.push(DecisionReason::SimulatedOpportunity),
        OpportunitySource::VerifiedLive { .. } => match opportunity.last_capture_at {
            None => reasons.push(DecisionReason::CaptureUnavailable),
            Some(at) if input.as_of - at >= Duration::seconds(MAX_CAPTURE_AGE_SECONDS) => {
                reasons.push(DecisionReason::CaptureStale);
            }
            Some(_) => (),
        },
    }
    match opportunity.activity {
        ActivityState::Unknown => reasons.push(DecisionReason::ActivityUnknown),
        ActivityState::Idle => (),
        ActivityState::UserSpeaking => reasons.push(DecisionReason::UserSpeaking),
        ActivityState::InteractionBusy => reasons.push(DecisionReason::InteractionBusy),
        ActivityState::OutputBusy => reasons.push(DecisionReason::OutputBusy),
        ActivityState::OutputHeld => reasons.push(DecisionReason::OutputHeld),
    }
    if opportunity.owner_assurance != OwnerAssurance::BoundOwner {
        reasons.push(DecisionReason::OwnerUnverified);
    }
    if opportunity.capability != OutputCapability::FollowUpVoice {
        reasons.push(DecisionReason::CapabilityUnavailable);
    }
    if budget.admitted_today >= policy.daily_budget {
        reasons.push(DecisionReason::DailyBudgetExhausted);
    }
    if budget
        .last_admitted_at
        .is_some_and(|at| input.as_of - at < Duration::seconds(i64::from(policy.cooldown_seconds)))
    {
        reasons.push(DecisionReason::Cooldown);
    }
    if budget.deduplicated {
        reasons.push(DecisionReason::Duplicate);
    }
    if policy.require_response_before_next && budget.awaiting_response {
        reasons.push(DecisionReason::AwaitingResponse);
    }
    if policy.mode == CompanionMode::Shadow {
        reasons.push(DecisionReason::ShadowMode);
    }
    if input.candidate.policy.policy.mode == CompanionMode::Shadow {
        reasons.push(DecisionReason::ShadowOrigin);
    }
    reasons.sort();
    let outcome = if reasons.iter().any(|reason| reason.suppresses()) {
        DecisionOutcome::Suppressed
    } else if !reasons.is_empty() {
        DecisionOutcome::ShadowWouldAllow
    } else {
        DecisionOutcome::Allowed
    };
    let mut valid_until = input.candidate.expires_at.min(opportunity.valid_until);
    if matches!(opportunity.source, OpportunitySource::VerifiedLive { .. })
        && let Some(captured_at) = opportunity.last_capture_at
    {
        valid_until = valid_until.min(captured_at + Duration::seconds(MAX_CAPTURE_AGE_SECONDS));
    }
    let next_utc_date = input
        .budget
        .utc_date
        .succ_opt()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|date| date.and_utc())
        .ok_or_else(|| anyhow::anyhow!("companion UTC budget window overflow"))?;
    valid_until = valid_until.min(next_utc_date);
    let mut record = DecisionRecord {
        schema_version: SCHEMA_VERSION,
        evaluator_version: DECISION_VERSION.into(),
        candidate_id: input.candidate.id,
        as_of: input.as_of,
        outcome,
        reasons,
        valid_until,
        input_digest: input.digest()?,
        result_digest: String::new(),
    };
    record.result_digest = result_digest(&record)?;
    record.validate()?;
    Ok(record)
}

/// Compare the recorded decision with the same frozen inputs. This deliberately
/// does not refresh policy, source validity, opportunities, or budget state.
pub fn replay(input: &DecisionInput, expected: &DecisionRecord) -> anyhow::Result<bool> {
    expected.validate()?;
    Ok(decide(input)? == *expected)
}

pub(super) fn result_digest(record: &DecisionRecord) -> anyhow::Result<String> {
    #[derive(Serialize)]
    struct ResultContent<'a> {
        schema_version: u16,
        evaluator_version: &'a str,
        candidate_id: Uuid,
        as_of: DateTime<Utc>,
        outcome: DecisionOutcome,
        reasons: &'a [DecisionReason],
        valid_until: DateTime<Utc>,
        input_digest: &'a str,
    }
    canonical_digest(&ResultContent {
        schema_version: record.schema_version,
        evaluator_version: &record.evaluator_version,
        candidate_id: record.candidate_id,
        as_of: record.as_of,
        outcome: record.outcome,
        reasons: &record.reasons,
        valid_until: record.valid_until,
        input_digest: &record.input_digest,
    })
}

#[cfg(test)]
mod tests;
