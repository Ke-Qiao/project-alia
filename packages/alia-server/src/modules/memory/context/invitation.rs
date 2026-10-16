//! Context-owned invitation evidence and immutable lease issuance.
//! The original source closure is locked before a live grant or its policy.
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{appraisal_source::lock_appraisal_authority, repository::*, *};
use crate::modules::companion::{
    invitation::{lock_invitation_grant, read_history_identity, read_invitation_grant},
    live::types::{INVITATION_KIND, INVITATION_OUTPUT_KEY, InvitationContent, LiveAdmissionGrant},
    source::load_companion_source,
};

fn revoked() -> anyhow::Error {
    ContextError::Revoked("Companion invitation authority is unavailable or changed".into()).into()
}

/// Keep every consumed source while excluding the authority to speak again.
pub(crate) async fn history_roots(
    tx: &DatabaseTransaction,
    lease: &ContextLease,
) -> ContextResult<(Vec<ContextEvidence>, Option<DateTime<Utc>>)> {
    if lease.purpose != ContextPurpose::CompanionInvitation
        || lease
            .dependencies
            .iter()
            .filter(|root| matches!(root, ContextEvidence::CompanionAdmission { .. }))
            .count()
            != 1
    {
        return Err(revoked());
    }
    let roots = lease
        .dependencies
        .iter()
        .filter(|root| !matches!(root, ContextEvidence::CompanionAdmission { .. }))
        .cloned()
        .collect::<Vec<_>>();
    let owner = lease.source.owner_id;
    let mut expiry = None;
    for root in &roots {
        let until = match root {
            ContextEvidence::UserMessage { .. }
            | ContextEvidence::MemorySourceMessage { .. }
            | ContextEvidence::Relationship { .. } => None,
            ContextEvidence::Memory { id, .. } => tx
                .query_one(stmt(
                    "SELECT expires_at FROM memory_items WHERE id=$1 AND user_id=$2 AND tenant_id=$3",
                    vec![(*id).into(), owner.into(), Uuid::nil().into()],
                ))
                .await?
                .ok_or_else(revoked)?
                .try_get("", "expires_at")?,
            ContextEvidence::Graph { edge_id, .. } => graph_projection(tx, owner, *edge_id)
                .await?
                .ok_or_else(revoked)?
                .2,
            ContextEvidence::Cache { id, .. } => {
                let cache = cache_projection(tx, owner, *id).await?.ok_or_else(revoked)?;
                earliest(
                    Some(cache.expires_at),
                    load(tx, owner, cache.context_id).await?.valid_until,
                )
            }
            ContextEvidence::DerivedMessage { context_id, .. } => {
                load(tx, owner, *context_id).await?.valid_until
            }
            ContextEvidence::Execution { effect_id, .. } => {
                let projection = execution_projection(tx, owner, *effect_id)
                    .await?
                    .ok_or_else(revoked)?;
                let parent = projection["context_id"]
                    .as_str()
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .ok_or_else(revoked)?;
                load(tx, owner, parent).await?.valid_until
            }
            ContextEvidence::Appraisal { id, .. }
            | ContextEvidence::AffectProjection { id, .. } => {
                let query = if matches!(root, ContextEvidence::Appraisal { .. }) {
                    "SELECT context_id,valid_until FROM appraisal_records WHERE appraisal_id=$1 AND owner_id=$2"
                } else {
                    "SELECT context_id,valid_until FROM affect_projections WHERE projection_id=$1 AND owner_id=$2"
                };
                let row = tx
                    .query_one(stmt(query, vec![(*id).into(), owner.into()]))
                    .await?
                    .ok_or_else(revoked)?;
                earliest(
                    row.try_get("", "valid_until")?,
                    load(tx, owner, row.try_get("", "context_id")?)
                        .await?
                        .valid_until,
                )
            }
            // Canonical Companion sources never consume an earlier invitation.
            ContextEvidence::CompanionAdmission { .. }
            | ContextEvidence::CompanionHistory { .. } => return Err(revoked()),
        };
        expiry = earliest(expiry, until);
    }
    ensure_live(tx, expiry).await?;
    Ok((roots, expiry))
}

pub(crate) async fn validate_history_reference(
    tx: &DatabaseTransaction,
    current: &ContextLease,
    message_id: Uuid,
    response_id: Uuid,
    context_id: Uuid,
    content_digest: &str,
) -> ContextResult<()> {
    let parent = load(tx, current.source.owner_id, context_id).await?;
    let (roots, _) = history_roots(tx, &parent).await?;
    if parent.policy_version != current.policy_version
        || parent.source.source_privacy_epoch != current.source.source_privacy_epoch
        || !roots.iter().all(|root| current.dependencies.contains(root))
    {
        return Err(revoked());
    }
    lock_authority(tx, &parent.source, parent.policy_version).await?;
    let admission_id = parent
        .dependencies
        .iter()
        .find_map(|root| match root {
            ContextEvidence::CompanionAdmission { id, .. } => Some(*id),
            _ => None,
        })
        .ok_or_else(revoked)?;
    // These admission columns cannot change or be deleted. History must not
    // acquire the live-grant lock while retirement requests Response cancellation.
    let candidate_id = read_history_identity(tx, &parent, admission_id).await?;
    let message = tx.query_one(stmt(
        "SELECT m.content FROM chat_messages m JOIN chat_sessions s ON s.id=m.session_id WHERE m.id=$1 AND m.session_id=$2 AND s.creator_id=$3 AND m.role='assistant' FOR SHARE OF m",
        vec![message_id.into(), parent.source.conversation_id.into(), current.source.owner_id.to_string().into()],
    )).await?.ok_or_else(revoked)?;
    let row = tx.query_one(stmt(
        "SELECT request_id,source_event_id,source_received_at,source_privacy_epoch,channel_id,content AS response_content,final_content FROM delivery_responses WHERE assistant_message_id=$1 AND response_id=$2 AND context_id=$3 AND user_id=$4 AND conversation_id=$5 AND status='committed' AND kind=$6 AND output_key=$7 FOR SHARE",
        vec![message_id.into(), response_id.into(), context_id.into(), current.source.owner_id.into(), parent.source.conversation_id.into(), INVITATION_KIND.into(), INVITATION_OUTPUT_KEY.into()],
    )).await?.ok_or_else(revoked)?;
    let content: String = message.try_get("", "content")?;
    let envelope: Value = row.try_get("", "response_content")?;
    let invitation: InvitationContent =
        serde_json::from_value(envelope.clone()).map_err(|_| revoked())?;
    invitation.validate().map_err(|_| revoked())?;
    if digest(&json!(content))? != content_digest
        || row.try_get::<Value>("", "final_content")? != json!({"text":content})
        || invitation.text != content
        || invitation.admission_id != admission_id
        || invitation.candidate_id != candidate_id
        || Some(invitation.source_event_id) != parent.source.source_event_id
        || digest(&envelope)? != parent.materialized_digest
        || row.try_get::<Uuid>("", "request_id")? != parent.source.request_id
        || row.try_get::<Option<Uuid>>("", "source_event_id")? != parent.source.source_event_id
        || row
            .try_get::<chrono::DateTime<chrono::Utc>>("", "source_received_at")?
            .timestamp_micros()
            != parent.source.source_received_at.timestamp_micros()
        || row.try_get::<Option<i64>>("", "source_privacy_epoch")?
            != Some(parent.source.source_privacy_epoch)
        || row.try_get::<String>("", "channel_id")? != parent.source.channel_id
    {
        return Err(revoked());
    }
    // Parent valid_until belongs to output admission. The roots above retain
    // their own expiry checks in validate_evidence, even after live retirement.
    Ok(())
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
mod history_tests;
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
