//! Context-owned invitation evidence and immutable lease issuance.
//! The original source closure is locked before a live grant or its policy.
use sea_orm::DatabaseTransaction;
use serde_json::json;
use uuid::Uuid;

use super::{appraisal_source::lock_appraisal_authority, repository::*, *};
use crate::modules::companion::{
    invitation::{lock_invitation_grant, read_invitation_grant},
    live::types::{INVITATION_KIND, LiveAdmissionGrant},
    source::load_companion_source,
};

fn revoked() -> anyhow::Error {
    ContextError::Revoked("Companion invitation authority is unavailable or changed".into()).into()
}

async fn invitation_lease(
    tx: &DatabaseTransaction,
    owner: Uuid,
    admission_id: Uuid,
) -> ContextResult<(LiveAdmissionGrant, ContextLease)> {
    // This first read acquires no grant lock. Source-erasure triggers take their
    // original root locks before retiring grants, so the reverse order deadlocks.
    let initial = read_invitation_grant(tx, owner, admission_id).await?;
    let authority = lock_appraisal_authority(tx, owner).await?;
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
    let (locked, locked_digest) = lock_invitation_grant(tx, owner, admission_id, &initial).await?;
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
    use chrono::DateTime;

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
