use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseTransaction, Statement};
use uuid::Uuid;

use crate::modules::memory::context::{ContextService, ContextUse};
use crate::providers::adapter::{MemoryWriteContext, MemoryWriteOrigin};

pub(super) fn context_use(origin: MemoryWriteOrigin) -> ContextUse {
    match origin {
        MemoryWriteOrigin::Manual => ContextUse::ManualMemoryWrite,
        MemoryWriteOrigin::Automatic => ContextUse::AutomaticMemoryWrite,
        MemoryWriteOrigin::Ai => ContextUse::AiMemoryWrite,
        MemoryWriteOrigin::Maintenance => ContextUse::MemoryMaintenance,
    }
}

pub(super) async fn lock_user(
    tx: &DatabaseTransaction,
    user: Uuid,
) -> anyhow::Result<MemoryWriteContext> {
    // Every domain takes locks in policy → account → privacy order. Holding
    // privacy while waiting for policy could deadlock a context validation.
    let policy =
        ContextService::policy_in_transaction(tx, None, ContextUse::ManualMemoryWrite).await?;
    let active = tx
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT id FROM users WHERE id=$1 AND status='active' FOR SHARE",
            [user.into()],
        ))
        .await?
        .is_some();
    anyhow::ensure!(active, "memory owner is not active");
    tx.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO memory_privacy_state(user_id) VALUES($1) ON CONFLICT DO NOTHING",
        [user.into()],
    ))
    .await?;
    let row = tx.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "SELECT privacy_epoch,source_cutoff_at,clock_timestamp() AS observed_at FROM memory_privacy_state WHERE user_id=$1 FOR UPDATE", [user.into()])).await?.ok_or_else(|| anyhow::anyhow!("memory privacy state missing"))?;
    let relationship_epoch =
        crate::modules::memory::source::lock_relationship_epoch(tx, user).await?;
    Ok(MemoryWriteContext {
        user_id: user,
        privacy_epoch: row.try_get("", "privacy_epoch")?,
        relationship_epoch: Some(relationship_epoch),
        policy_version: Some(policy.version),
        context_id: None,
        origin: MemoryWriteOrigin::Manual,
        source_cutoff_at: row.try_get("", "source_cutoff_at")?,
        source_received_at: row.try_get("", "observed_at")?,
        source_event_ids: Vec::new(),
        exact_source_received_at: None,
        source_memories: Vec::new(),
    })
}

pub(super) async fn validate(
    tx: &DatabaseTransaction,
    expected: &MemoryWriteContext,
) -> anyhow::Result<()> {
    validate_sources(tx, expected, true).await
}

pub(super) async fn validate_forget(
    tx: &DatabaseTransaction,
    expected: &MemoryWriteContext,
) -> anyhow::Result<()> {
    validate_sources(tx, expected, false).await
}

async fn validate_sources(
    tx: &DatabaseTransaction,
    expected: &MemoryWriteContext,
    require_live: bool,
) -> anyhow::Result<()> {
    let version = expected
        .policy_version
        .ok_or_else(|| anyhow::anyhow!("database memory write has no persistent policy version"))?;
    let purpose = context_use(expected.origin);
    ContextService::policy_in_transaction(tx, Some(version), purpose).await?;
    if let Some(id) = expected.context_id {
        let validated =
            ContextService::validate_lease_in_transaction(tx, expected.user_id, id, purpose)
                .await?;
        let canonical = validated
            .lease
            .memory_write_context(expected.clone(), expected.origin)?;
        anyhow::ensure!(
            canonical.source_received_at.timestamp_micros()
                == expected.source_received_at.timestamp_micros()
                && canonical
                    .exact_source_received_at
                    .map(|(id, at)| (id, at.timestamp_micros()))
                    == expected
                        .exact_source_received_at
                        .map(|(id, at)| (id, at.timestamp_micros()))
                && canonical
                    .source_event_ids
                    .iter()
                    .all(|id| expected.source_event_ids.contains(id))
                && canonical
                    .source_memories
                    .iter()
                    .all(|source| expected.source_memories.contains(source)),
            "derived memory write omitted or replaced canonical context roots"
        );
    } else {
        anyhow::ensure!(
            !matches!(
                expected.origin,
                MemoryWriteOrigin::Ai | MemoryWriteOrigin::Automatic
            ),
            "model-derived memory write requires its original context lease"
        );
    }
    let current = lock_user(tx, expected.user_id).await?;
    anyhow::ensure!(
        current.privacy_epoch == expected.privacy_epoch,
        "memory privacy epoch changed"
    );
    if let Some((id, _)) = expected.exact_source_received_at {
        anyhow::ensure!(
            expected.source_event_ids.contains(&id),
            "exact source must be part of the verified source set"
        );
    }
    for id in &expected.source_event_ids {
        let source = tx.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT m.source_received_at FROM chat_messages m JOIN chat_sessions s ON s.id=m.session_id WHERE m.id=$1 AND s.creator_id=$2 AND m.role='user' AND m.user_id=$2 AND m.source_received_at IS NOT NULL AND m.source_privacy_epoch=$3",
            vec![(*id).into(),expected.user_id.to_string().into(),current.privacy_epoch.into()])).await?
            .ok_or_else(|| anyhow::anyhow!("conversation source was erased, belongs to another privacy epoch, is unsourced, or belongs to another user"))?;
        if let Some((exact_id, received_at)) = expected.exact_source_received_at
            && exact_id == *id
        {
            let stored: DateTime<Utc> = source.try_get("", "source_received_at")?;
            anyhow::ensure!(
                stored.timestamp_micros() == received_at.timestamp_micros(),
                "conversation source timestamp does not match the original message"
            );
        }
    }
    for (id, revision) in &expected.source_memories {
        let valid = tx.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT id FROM memory_items WHERE id=$1::uuid AND user_id=$2 AND tenant_id=$3 AND revision=$4 AND status<>'deleted' AND (NOT $5 OR (status='active' AND (expires_at IS NULL OR expires_at>clock_timestamp())))",
            vec![id.clone().into(), expected.user_id.into(), Uuid::nil().into(), (*revision).into(), require_live.into()])).await?.is_some();
        anyhow::ensure!(
            valid,
            "memory source was changed, expired, deleted, or belongs to another user"
        );
    }
    Ok(())
}

pub(super) async fn erase(
    tx: &DatabaseTransaction,
    user: Uuid,
    ids: &[String],
) -> anyhow::Result<u64> {
    // Caller owns the user row lock. Advancing even for an empty clear/topic
    // result invalidates work that has not produced its first record yet.
    tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "UPDATE memory_privacy_state SET privacy_epoch=privacy_epoch+1,source_cutoff_at=clock_timestamp() WHERE user_id=$1", [user.into()])).await?;
    tx.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM llm_cache WHERE user_id=$1",
        [user.into()],
    ))
    .await?;
    tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "UPDATE delivery_responses SET cancel_requested=TRUE WHERE user_id=$1 AND status='pending' AND NOT cancel_requested", [user.into()])).await?;
    let rows = tx.query_all(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "WITH RECURSIVE erased(id) AS (SELECT id FROM memory_items WHERE user_id=$1 AND tenant_id=$2 AND id=ANY(ARRAY(SELECT jsonb_array_elements_text($3::jsonb)::uuid)) UNION SELECT s.memory_id FROM memory_sources s JOIN erased e ON s.source_id=e.id JOIN memory_items m ON m.id=s.memory_id WHERE m.user_id=$1 AND m.tenant_id=$2) SELECT id::text FROM erased",
        vec![user.into(), Uuid::nil().into(), serde_json::json!(ids).into()])).await?;
    let erased = rows
        .into_iter()
        .map(|row| row.try_get::<String>("", "id"))
        .collect::<Result<Vec<_>, _>>()?;
    for id in &erased {
        tx.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM memory_embeddings_1536 WHERE memory_id=$1::uuid",
            [id.clone().into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "DELETE FROM memory_edges WHERE user_id=$2 AND (evidence_memory_id=$1::uuid OR src_node_id IN (SELECT id FROM memory_nodes WHERE user_id=$2 AND node_key='memory:'||$1) OR dst_node_id IN (SELECT id FROM memory_nodes WHERE user_id=$2 AND node_key='memory:'||$1))", vec![id.clone().into(), user.into()])).await?;
        tx.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM memory_nodes WHERE user_id=$2 AND node_key='memory:'||$1",
            vec![id.clone().into(), user.into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM memory_sources WHERE memory_id=$1::uuid OR source_id=$1::uuid",
            [id.clone().into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE memory_items SET status='deleted',revision=revision+1,content='',summary=NULL,evidence='{}',metadata='{}',topic_tags='{}',affect_tags='{}',source_event_ids='{}',superseded_by=NULL,updated_at=clock_timestamp(),expires_at=clock_timestamp() WHERE id=$1::uuid AND user_id=$2 AND status<>'deleted'", vec![id.clone().into(), user.into()])).await?;
    }
    tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "DELETE FROM memory_nodes n WHERE n.user_id=$1 AND NOT EXISTS(SELECT 1 FROM memory_edges e WHERE e.src_node_id=n.id OR e.dst_node_id=n.id)", [user.into()])).await?;
    Ok(erased.len() as u64)
}

pub(super) fn initial_context(user: Uuid) -> MemoryWriteContext {
    MemoryWriteContext {
        relationship_epoch: None,
        user_id: user,
        privacy_epoch: 0,
        policy_version: None,
        context_id: None,
        origin: MemoryWriteOrigin::Manual,
        source_cutoff_at: DateTime::UNIX_EPOCH,
        source_received_at: Utc::now(),
        source_event_ids: Vec::new(),
        exact_source_received_at: None,
        source_memories: Vec::new(),
    }
}
