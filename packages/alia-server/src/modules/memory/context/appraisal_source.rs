//! Canonical user-appraisal evidence read under the Context authority locks.
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction};
use uuid::Uuid;

use super::{repository::*, *};

pub(crate) struct AppraisalReadAuthority {
    pub context_policy_version: i64,
    pub privacy_epoch: i64,
    pub relationship_epoch: i64,
    pub as_of: DateTime<Utc>,
    pub private_readable: bool,
}

/// The same authority lock order as Context writers and destructive operations.
pub(crate) async fn lock_appraisal_authority(
    tx: &DatabaseTransaction,
    owner: Uuid,
) -> ContextResult<AppraisalReadAuthority> {
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
    Ok(AppraisalReadAuthority {
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

pub(crate) struct UserAppraisalSource {
    pub lease: ContextLease,
    pub appraisal_id: Uuid,
    pub revision: i64,
    pub result_digest: String,
    pub effective_at: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
}

pub(crate) async fn load_user_appraisal_source(
    tx: &DatabaseTransaction,
    owner: Uuid,
    appraisal_id: Uuid,
    authority: &AppraisalReadAuthority,
) -> ContextResult<UserAppraisalSource> {
    if !authority.private_readable {
        return Err(ContextError::Forbidden.into());
    }
    let revoked = || ContextError::Revoked("appraisal source is unavailable".into());
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
            "source requires a canonical user-message appraisal".into(),
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
    let revision = row.try_get("", "revision")?;
    let result_digest: String = row.try_get("", "result_digest")?;
    merge(
        &mut lease.dependencies,
        &[
            ContextEvidence::Relationship {
                epoch: authority.relationship_epoch,
            },
            ContextEvidence::Appraisal {
                id: appraisal_id,
                revision,
                result_digest: result_digest.clone(),
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
        return Err(ContextError::Revoked("appraisal user source was re-timed".into()).into());
    }
    source.source_event_id.ok_or_else(revoked)?;
    source.conversation_id.ok_or_else(revoked)?;
    Ok(UserAppraisalSource {
        lease,
        appraisal_id,
        revision,
        result_digest,
        effective_at,
        valid_until: until,
    })
}
