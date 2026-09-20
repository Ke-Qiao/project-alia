//! Owner-scoped relationship reads and destructive operations share authority
//! locks with every derived writer. A clear receipt is durable and replayable.
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction, TransactionTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{repository::*, *};
use crate::expression::{AppraisalScope, RelationshipProfile, RelationshipPruneResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelationshipClearReceipt {
    pub operation_id: Uuid,
    pub owner_id: Uuid,
    pub previous_epoch: i64,
    pub relationship_epoch: i64,
    pub removed_appraisals: u64,
    pub removed_memories: u64,
}

async fn lock_owner(
    tx: &DatabaseTransaction,
    owner: Uuid,
) -> ContextResult<(PolicySnapshot, i64, i64)> {
    let policy = policy::verify(tx, None, ContextUse::AppraisalMaintenance).await?;
    if tx
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
    let privacy = tx
        .query_one(stmt(
            "SELECT privacy_epoch FROM memory_privacy_state WHERE user_id=$1 FOR UPDATE",
            vec![owner.into()],
        ))
        .await?
        .unwrap()
        .try_get("", "privacy_epoch")?;
    let relationship = appraisal::relationship_epoch(tx, owner).await?;
    Ok((policy, privacy, relationship))
}

impl ContextService {
    pub async fn relationship_profile(&self, owner: Uuid) -> ContextResult<RelationshipProfile> {
        let tx = self.db.begin().await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let (policy, privacy, epoch) = lock_owner(&tx, owner).await?;
        anyhow::ensure!(
            policy.policy.appraisal.reducer_version == AFFECT_REDUCER_VERSION,
            "configured relationship reducer is unavailable"
        );
        if !policy
            .policy
            .appraisal
            .affect
            .relationship_profile_user_visible
        {
            return Err(ContextError::Forbidden.into());
        }
        let now = tx
            .query_one(stmt("SELECT clock_timestamp() AS now", vec![]))
            .await?
            .unwrap()
            .try_get::<DateTime<Utc>>("", "now")?;
        let rows=tx.query_all(stmt("SELECT * FROM appraisal_records WHERE owner_id=$1 AND source_privacy_epoch=$2 AND source_relationship_epoch=$3 AND policy_version=$4 AND status='active' AND effective_at<=$5 AND valid_until>$5 ORDER BY effective_at DESC,appraisal_id LIMIT 256",vec![owner.into(),privacy.into(),epoch.into(),policy.version.into(),now.into()])).await?;
        let mut events = Vec::new();
        if policy.policy.appraisal.affect.enabled
            && policy
                .policy
                .appraisal
                .affect
                .relationship_persistence_enabled
        {
            for row in rows {
                let mut lease = load(&tx, owner, row.try_get("", "context_id")?).await?;
                let batch = appraisal::record_events(&row)?;
                let selected = batch
                    .into_iter()
                    .filter(|e| {
                        e.is_active(now)
                            && matches!(
                                e.scope,
                                AppraisalScope::User | AppraisalScope::Relationship
                            )
                    })
                    .collect::<Vec<_>>();
                if selected.is_empty()
                    || selected.len() + events.len()
                        > policy.policy.appraisal.affect.max_active_appraisal_events
                {
                    continue;
                }
                merge(
                    &mut lease.dependencies,
                    &[
                        ContextEvidence::Relationship { epoch },
                        ContextEvidence::Appraisal {
                            id: row.try_get("", "appraisal_id")?,
                            revision: row.try_get("", "revision")?,
                            result_digest: row.try_get("", "result_digest")?,
                        },
                    ],
                )?;
                match validate_evidence(&tx, &lease).await {
                    Ok(()) => events.extend(selected),
                    Err(error) if super::appraisal::is_revoked(&error) => continue,
                    Err(error) => return Err(error),
                }
            }
        }
        let (_, mut profile) = crate::expression::affect::reduce_validated_events(
            owner,
            events,
            &policy.policy.appraisal.affect,
            now,
        );
        profile.relationship_epoch = epoch;
        tx.commit().await?;
        Ok(profile)
    }

    pub async fn clear_relationship(
        &self,
        owner: Uuid,
        operation_id: Uuid,
        expected_epoch: i64,
    ) -> ContextResult<RelationshipClearReceipt> {
        anyhow::ensure!(
            !owner.is_nil() && !operation_id.is_nil() && expected_epoch >= 0,
            "invalid relationship clear identity"
        );
        let tx = self.db.begin().await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let (policy, _, epoch) = lock_owner(&tx, owner).await?;
        if let Some(row) = tx
            .query_one(stmt(
                "SELECT * FROM relationship_clear_operations WHERE operation_id=$1 FOR SHARE",
                vec![operation_id.into()],
            ))
            .await?
        {
            if row.try_get::<Uuid>("", "owner_id")? != owner
                || row.try_get::<i64>("", "expected_epoch")? != expected_epoch
            {
                return Err(ContextError::RelationshipOperationConflict { operation_id }.into());
            }
            let receipt = RelationshipClearReceipt {
                operation_id,
                owner_id: owner,
                previous_epoch: expected_epoch,
                relationship_epoch: row.try_get("", "resulting_epoch")?,
                removed_appraisals: row.try_get::<i64>("", "removed_records")? as u64,
                removed_memories: row.try_get::<i64>("", "removed_memories")? as u64,
            };
            tx.commit().await?;
            return Ok(receipt);
        }
        if !policy
            .policy
            .appraisal
            .affect
            .relationship_profile_user_clearable
        {
            return Err(ContextError::Forbidden.into());
        }
        if epoch != expected_epoch {
            return Err(ContextError::RelationshipConflict {
                expected: expected_epoch,
                actual: epoch,
            }
            .into());
        }
        let next = epoch
            .checked_add(1)
            .ok_or_else(|| ContextError::Unavailable("relationship epoch exhausted".into()))?;
        tx.execute(stmt("UPDATE relationship_authority SET relationship_epoch=$2,updated_at=clock_timestamp() WHERE owner_id=$1",vec![owner.into(),next.into()])).await?;
        // Physically erase only memories derived from this domain, including
        // their memory descendants. Independent memories keep their epoch.
        let removed_memories =
            super::memory_appraisal::erase_relationship_memories(&tx, owner).await?;
        tx.execute(stmt(
            "DELETE FROM affect_projections WHERE owner_id=$1",
            vec![owner.into()],
        ))
        .await?;
        let removed_appraisals = tx
            .execute(stmt(
                "DELETE FROM appraisal_records WHERE owner_id=$1",
                vec![owner.into()],
            ))
            .await?
            .rows_affected();
        tx.execute(stmt(
            "DELETE FROM llm_cache WHERE user_id=$1",
            vec![owner.into()],
        ))
        .await?;
        tx.execute(stmt("UPDATE delivery_responses r SET cancel_requested=TRUE WHERE r.user_id=$1 AND r.status='pending' AND EXISTS(SELECT 1 FROM context_leases c,jsonb_array_elements(c.dependencies) d WHERE c.context_id=r.context_id AND d->>'kind' IN ('relationship','appraisal','affect_projection'))",vec![owner.into()])).await?;
        tx.execute(stmt("DELETE FROM conversation_events e USING chat_sessions s WHERE e.session_id=s.id AND s.creator_id=$1 AND e.actor IN ('affect','expression')",vec![owner.to_string().into()])).await?;
        tx.execute(stmt("INSERT INTO relationship_clear_operations(operation_id,owner_id,expected_epoch,resulting_epoch,removed_records,removed_memories) VALUES($1,$2,$3,$4,$5,$6)",vec![operation_id.into(),owner.into(),epoch.into(),next.into(),(removed_appraisals as i64).into(),(removed_memories as i64).into()])).await?;
        tx.commit()
            .await
            .map_err(|_| ContextError::CommitUnknownOperation { operation_id })?;
        Ok(RelationshipClearReceipt {
            operation_id,
            owner_id: owner,
            previous_epoch: epoch,
            relationship_epoch: next,
            removed_appraisals,
            removed_memories,
        })
    }

    pub async fn prune_relationship(
        &self,
        owner: Uuid,
        limit: u16,
    ) -> ContextResult<RelationshipPruneResult> {
        anyhow::ensure!(
            (1..=256).contains(&limit),
            "relationship prune limit must be 1..256"
        );
        let tx = self.db.begin().await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        lock_owner(&tx, owner).await?;
        let profiles=tx.execute(stmt("DELETE FROM affect_projections WHERE projection_id IN (SELECT projection_id FROM affect_projections WHERE owner_id=$1 AND valid_until<=clock_timestamp() ORDER BY valid_until,projection_id LIMIT $2)",vec![owner.into(),i64::from(limit).into()])).await?.rows_affected();
        let records=tx.execute(stmt("DELETE FROM appraisal_records WHERE appraisal_id IN (SELECT appraisal_id FROM appraisal_records WHERE owner_id=$1 AND valid_until<=clock_timestamp() ORDER BY valid_until,appraisal_id LIMIT $2)",vec![owner.into(),i64::from(limit).into()])).await?.rows_affected();
        tx.commit().await?;
        Ok(RelationshipPruneResult {
            profiles_removed: profiles as usize,
            relationship_states_removed: records as usize,
        })
    }
}
