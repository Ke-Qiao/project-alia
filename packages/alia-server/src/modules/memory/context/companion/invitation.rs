//! The original source closure is locked before a live grant or its policy.
use super::*;
use crate::modules::companion::{
    live::types::{INVITATION_KIND, INVITATION_OUTPUT_KEY, LiveAdmissionGrant},
    types::{CompanionMode, CompanionPolicy, PolicySnapshot as CompanionPolicySnapshot},
};
use serde_json::{Value, json};

fn revoked() -> anyhow::Error {
    ContextError::Revoked("Companion invitation authority is unavailable or changed".into()).into()
}

async fn grant(
    tx: &DatabaseTransaction,
    owner: Uuid,
    admission_id: Uuid,
    lock: bool,
) -> ContextResult<(LiveAdmissionGrant, String)> {
    let query = if lock {
        "SELECT * FROM companion_admissions WHERE admission_id=$1 AND owner_id=$2 AND state='active' FOR SHARE"
    } else {
        "SELECT * FROM companion_admissions WHERE admission_id=$1 AND owner_id=$2 AND state='active'"
    };
    let row = tx
        .query_one(stmt(query, vec![admission_id.into(), owner.into()]))
        .await?
        .ok_or_else(revoked)?;
    let value = row
        .try_get::<Option<Value>>("", "private_grant")?
        .ok_or_else(revoked)?;
    let stored_digest = row
        .try_get::<Option<String>>("", "grant_digest")?
        .ok_or_else(revoked)?;
    if serde_json::to_vec(&value)?.len() > 64 * 1024 || digest(&value)? != stored_digest {
        return Err(revoked());
    }
    let grant: LiveAdmissionGrant = serde_json::from_value(value).map_err(|_| revoked())?;
    grant.validate().map_err(|_| revoked())?;
    let charged_at: DateTime<Utc> = row.try_get("", "charged_at")?;
    if grant.digest()? != stored_digest
        || grant.admission_id != admission_id
        || grant.candidate.observation.owner_id != owner
        || row.try_get::<Uuid>("", "candidate_id")? != grant.candidate.id
        || row.try_get::<Uuid>("", "initiative_request_id")? != grant.initiative_request_id
        || row.try_get::<Option<Uuid>>("", "context_id")? != Some(grant.context_id)
        || row.try_get::<Uuid>("", "server_instance_id")? != grant.server_instance_id
        || row.try_get::<Uuid>("", "opportunity_id")? != grant.opportunity.id
        || row.try_get::<Uuid>("", "conversation_id")?
            != grant.candidate.observation.conversation_id
        || row
            .try_get::<Option<String>>("", "intent_digest")?
            .as_deref()
            != Some(grant.intent_digest.as_str())
        || row.try_get::<String>("", "output_key")? != INVITATION_OUTPUT_KEY
        || row.try_get::<DateTime<Utc>>("", "admit_before")? != grant.admit_before
        || row.try_get::<DateTime<Utc>>("", "valid_until")? != grant.valid_until
        || u64::try_from(row.try_get::<i64>("", "budget_revision")?).ok()
            != grant.input.budget.revision.checked_add(1)
        || charged_at != grant.input.as_of
        || charged_at >= grant.admit_before
        || row.try_get::<chrono::NaiveDate>("", "budget_date")? != charged_at.date_naive()
        || grant.initiative_request_id == grant.candidate.observation.request_id
        || grant.context_id == grant.candidate.observation.context_id
    {
        return Err(revoked());
    }
    Ok((grant, stored_digest))
}

/// Read immutable binding facts after the caller has validated the dedicated
/// Context in this transaction. This read alone grants no output permission.
pub(crate) async fn read_invitation_grant(
    tx: &DatabaseTransaction,
    owner: Uuid,
    admission_id: Uuid,
) -> ContextResult<LiveAdmissionGrant> {
    Ok(grant(tx, owner, admission_id, false).await?.0)
}

async fn invitation_lease(
    tx: &DatabaseTransaction,
    owner: Uuid,
    admission_id: Uuid,
) -> ContextResult<(LiveAdmissionGrant, ContextLease)> {
    // This first read acquires no grant lock. Source-erasure triggers take their
    // original root locks before retiring grants, so the reverse order deadlocks.
    let (initial, initial_digest) = grant(tx, owner, admission_id, false).await?;
    let authority = lock_companion_authority(tx, owner).await?;
    let references = &initial.candidate.observation.appraisals;
    let mut canonical =
        load_companion_source(tx, owner, references[0].appraisal_id, &authority).await?;
    for reference in &references[1..] {
        let other = load_companion_source(tx, owner, reference.appraisal_id, &authority).await?;
        if canonical.lease.id != other.lease.id || canonical.lease.source != other.lease.source {
            return Err(revoked());
        }
        canonical.observation.valid_until = canonical
            .observation
            .valid_until
            .min(other.observation.valid_until);
        canonical
            .observation
            .appraisals
            .extend(other.observation.appraisals);
        merge(&mut canonical.lease.dependencies, &other.lease.dependencies)?;
    }
    if canonical.observation.digest()? != initial.candidate.observation.digest()? {
        return Err(revoked());
    }
    let row = tx
        .query_one(stmt(
            "SELECT revision,policy,policy_digest FROM companion_policies WHERE owner_id=$1 FOR SHARE",
            vec![owner.into()],
        ))
        .await?
        .ok_or_else(revoked)?;
    let policy: CompanionPolicy =
        serde_json::from_value(row.try_get("", "policy")?).map_err(|_| revoked())?;
    let current = CompanionPolicySnapshot {
        revision: row.try_get("", "revision")?,
        policy,
    };
    current.validate().map_err(|_| revoked())?;
    if current != initial.candidate.policy
        || current.policy.digest()? != row.try_get::<String>("", "policy_digest")?
        || current.policy.mode != CompanionMode::Enabled
        || current.policy.quiet
        || current.policy.paused
    {
        return Err(revoked());
    }
    let (locked, locked_digest) = grant(tx, owner, admission_id, true).await?;
    if locked_digest != initial_digest || locked != initial {
        return Err(revoked());
    }
    ensure_live(tx, Some(locked.valid_until)).await?;
    let mut lease = canonical.lease;
    lease.id = locked.context_id;
    lease.purpose = ContextPurpose::CompanionInvitation;
    lease.source.request_id = locked.initiative_request_id;
    lease.valid_until = Some(locked.valid_until);
    lease.materialized_digest = locked.content.digest()?;
    merge(
        &mut lease.dependencies,
        &[ContextEvidence::CompanionAdmission {
            id: admission_id,
            intent_digest: locked_digest,
        }],
    )?;
    Ok((locked, lease))
}

pub(crate) async fn issue_invitation_in_transaction(
    tx: &DatabaseTransaction,
    owner: Uuid,
    admission_id: Uuid,
) -> ContextResult<ContextLease> {
    let (grant, mut lease) = invitation_lease(tx, owner, admission_id).await?;
    // Issuance is the first admission, not restoration of an existing producer.
    // Later validation deliberately uses only the immutable output deadline.
    ensure_live(tx, Some(grant.admit_before)).await?;
    persist(
        tx,
        &mut lease,
        INVITATION_KIND,
        INVITATION_KIND,
        &json!(grant.content),
    )
    .await?;
    if lease.id != grant.context_id {
        return Err(revoked());
    }
    Ok(lease)
}

pub(crate) async fn validate_invitation_reference(
    tx: &DatabaseTransaction,
    lease: &ContextLease,
    admission_id: Uuid,
    intent_digest: &str,
) -> ContextResult<()> {
    let (_, expected) = invitation_lease(tx, lease.source.owner_id, admission_id).await?;
    if lease.id != expected.id
        || lease.purpose != expected.purpose
        || lease.source != expected.source
        || lease.policy_version != expected.policy_version
        || lease.materialized_digest != expected.materialized_digest
        || lease.valid_until != expected.valid_until
        || lease.dependencies != expected.dependencies
        || !expected
            .dependencies
            .contains(&ContextEvidence::CompanionAdmission {
                id: admission_id,
                intent_digest: intent_digest.into(),
            })
    {
        return Err(revoked());
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod purpose_tests {
    use super::*;

    #[test]
    fn invitation_authorizes_only_its_two_output_operations() {
        let lease = ContextLease {
            id: Uuid::from_u128(1),
            purpose: ContextPurpose::CompanionInvitation,
            materialized_digest: "0".repeat(64),
            source: ContextSource {
                owner_id: Uuid::from_u128(2),
                request_id: Uuid::from_u128(3),
                channel_id: "fixture".into(),
                conversation_id: Some(Uuid::from_u128(4)),
                source_event_id: Some(Uuid::from_u128(5)),
                source_received_at: DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
                source_privacy_epoch: 0,
            },
            policy_version: 1,
            dependencies: vec![],
            valid_until: None,
        };
        assert!(lease.authorizes(ContextUse::ResponseCommit));
        assert!(lease.authorizes(ContextUse::OutputAdmission));
        for usage in [
            ContextUse::AutomaticMemoryWrite,
            ContextUse::AiMemoryWrite,
            ContextUse::ManualMemoryWrite,
            ContextUse::MemoryMaintenance,
            ContextUse::MemoryEmbedding,
            ContextUse::CacheWrite,
            ContextUse::CacheReuse,
            ContextUse::CacheReview,
            ContextUse::CacheEmbedding,
            ContextUse::Execution,
            ContextUse::AppraisalWrite,
            ContextUse::AffectRead,
            ContextUse::AppraisalMaintenance,
        ] {
            assert!(!lease.authorizes(usage), "unexpected authority: {usage:?}");
        }
    }
}
