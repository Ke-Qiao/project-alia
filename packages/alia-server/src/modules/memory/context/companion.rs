//! Canonical Companion source reads and narrowly scoped invitation Contexts.
//! A generation/rebuild capability cannot authorize a new invitation by itself.
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction};
use uuid::Uuid;

use super::{repository::*, *};
use crate::modules::companion::types::{AppraisalReference, CanonicalObservation};

mod invitation;
pub(crate) use invitation::{
    issue_invitation_in_transaction, read_invitation_grant, validate_invitation_reference,
};

pub(crate) struct CompanionAuthority {
    pub context_policy_version: i64,
    pub privacy_epoch: i64,
    pub relationship_epoch: i64,
    pub as_of: DateTime<Utc>,
    pub private_readable: bool,
}

/// The same authority lock order as Context writers and destructive operations.
pub(crate) async fn lock_companion_authority(
    tx: &DatabaseTransaction,
    owner: Uuid,
) -> ContextResult<CompanionAuthority> {
    let policy = policy::read(tx, "FOR SHARE").await?;
    if owner.is_nil()
        || tx
            .query_one(stmt(
                "SELECT id FROM users WHERE id=$1 AND status='active' FOR SHARE",
                vec![owner.into()],
            ))
            .await?
            .is_none()
    {
        return Err(ContextError::Forbidden.into());
    }
    tx.execute(stmt(
        "INSERT INTO memory_privacy_state(user_id) VALUES($1) ON CONFLICT DO NOTHING",
        vec![owner.into()],
    ))
    .await?;
    let privacy_epoch = tx
        .query_one(stmt(
            "SELECT privacy_epoch FROM memory_privacy_state WHERE user_id=$1 FOR UPDATE",
            vec![owner.into()],
        ))
        .await?
        .unwrap()
        .try_get("", "privacy_epoch")?;
    let relationship_epoch = appraisal::relationship_epoch(tx, owner).await?;
    let as_of = tx
        .query_one(stmt("SELECT clock_timestamp() AS now", vec![]))
        .await?
        .unwrap()
        .try_get("", "now")?;
    Ok(CompanionAuthority {
        context_policy_version: policy.version,
        privacy_epoch,
        relationship_epoch,
        as_of,
        private_readable: policy.policy.appraisal.affect.enabled
            && policy
                .policy
                .appraisal
                .affect
                .relationship_profile_user_visible
            && policy.policy.paused_until.is_none(),
    })
}

pub(crate) struct CompanionSource {
    pub observation: CanonicalObservation,
    /// Original parent identity with every flattened canonical dependency.
    pub lease: ContextLease,
    /// Opaque references for transactional erasure, never copied source text.
    pub erasure_roots: Vec<(&'static str, Uuid)>,
}

pub(crate) async fn load_companion_source(
    tx: &DatabaseTransaction,
    owner: Uuid,
    appraisal_id: Uuid,
    authority: &CompanionAuthority,
) -> ContextResult<CompanionSource> {
    if !authority.private_readable {
        return Err(ContextError::Forbidden.into());
    }
    let revoked = || ContextError::Revoked("Companion source is unavailable".into());
    let row = tx.query_one(stmt("SELECT * FROM appraisal_records WHERE appraisal_id=$1 AND owner_id=$2 AND status='active' FOR SHARE", vec![appraisal_id.into(), owner.into()])).await?.ok_or_else(revoked)?;
    let effective_at: DateTime<Utc> = row.try_get("", "effective_at")?;
    if row.try_get::<i64>("", "policy_version")? != authority.context_policy_version
        || row.try_get::<i64>("", "source_privacy_epoch")? != authority.privacy_epoch
        || row.try_get::<i64>("", "source_relationship_epoch")? != authority.relationship_epoch
        || effective_at > authority.as_of
    {
        return Err(revoked().into());
    }
    let fact: AppraisalFact = serde_json::from_value(row.try_get("", "source_fact")?)?;
    if !matches!(fact, AppraisalFact::UserMessage { .. }) {
        return Err(ContextError::Revoked(
            "the first Companion intention requires a canonical user-message appraisal".into(),
        )
        .into());
    }
    let mut lease = load(tx, owner, row.try_get("", "context_id")?).await?;
    lock_authority(tx, &lease.source, authority.context_policy_version).await?;
    ensure_live(tx, lease.valid_until).await?;
    let mut until: DateTime<Utc> = row.try_get("", "valid_until")?;
    if let Some(parent) = lease.valid_until {
        until = until.min(parent);
    }
    ensure_live(tx, Some(until)).await?;
    let reference = AppraisalReference {
        appraisal_id,
        revision: row.try_get("", "revision")?,
        result_digest: row.try_get("", "result_digest")?,
    };
    merge(
        &mut lease.dependencies,
        &[
            ContextEvidence::Relationship {
                epoch: authority.relationship_epoch,
            },
            ContextEvidence::Appraisal {
                id: appraisal_id,
                revision: reference.revision,
                result_digest: reference.result_digest.clone(),
            },
        ],
    )?;
    // Includes record digest, immutable parent header, source fact, and every
    // flattened Memory/graph/cache/execution ancestor consumed by that record.
    validate_evidence(tx, &lease).await?;
    let source = &lease.source;
    // User-message appraisal preparation and explicit rebuild both retain this
    // exact original timestamp. A rewritten Appraisal cannot refresh a source.
    if effective_at != source.source_received_at {
        return Err(ContextError::Revoked("Companion user source was re-timed".into()).into());
    }
    let source_event_id = source.source_event_id.ok_or_else(revoked)?;
    let conversation_id = source.conversation_id.ok_or_else(revoked)?;
    let mut roots = vec![("appraisal", appraisal_id), ("message", source_event_id)];
    for evidence in &lease.dependencies {
        match evidence {
            ContextEvidence::UserMessage { id, .. }
            | ContextEvidence::MemorySourceMessage { id, .. } => roots.push(("message", *id)),
            ContextEvidence::DerivedMessage {
                id, response_id, ..
            } => {
                roots.push(("message", *id));
                roots.push(("response", *response_id));
            }
            ContextEvidence::Memory { id, .. } => roots.push(("memory", *id)),
            ContextEvidence::Graph { edge_id, .. } => roots.push(("graph", *edge_id)),
            ContextEvidence::Cache { id, .. } => roots.push(("cache", *id)),
            ContextEvidence::Execution { effect_id, .. } => roots.push(("execution", *effect_id)),
            ContextEvidence::Appraisal { id, .. } => roots.push(("appraisal", *id)),
            ContextEvidence::AffectProjection { id, .. } => roots.push(("affect", *id)),
            ContextEvidence::Relationship { .. } => (),
            ContextEvidence::CompanionAdmission { .. } => {
                return Err(ContextError::Revoked(
                    "an invitation cannot become a new Companion source".into(),
                )
                .into());
            }
        }
    }
    match fact {
        AppraisalFact::ResponseCommitted { response_id, .. } => {
            roots.push(("response", response_id))
        }
        AppraisalFact::Execution { effect_id, .. } => roots.push(("execution", effect_id)),
        _ => (),
    }
    roots.sort_unstable();
    roots.dedup();
    Ok(CompanionSource {
        observation: CanonicalObservation {
            owner_id: owner,
            context_id: lease.id,
            request_id: source.request_id,
            source_event_id,
            conversation_id,
            channel_id: source.channel_id.clone(),
            source_received_at: source.source_received_at,
            effective_at,
            privacy_epoch: authority.privacy_epoch,
            relationship_epoch: authority.relationship_epoch,
            context_policy_version: authority.context_policy_version,
            appraisals: vec![reference],
            valid_until: until,
        },
        erasure_roots: roots,
        lease,
    })
}
