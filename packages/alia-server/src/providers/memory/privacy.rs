use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseTransaction, Statement};
use uuid::Uuid;

use crate::providers::adapter::MemoryWriteContext;

pub(super) async fn lock_user(
    tx: &DatabaseTransaction,
    user: Uuid,
) -> anyhow::Result<MemoryWriteContext> {
    tx.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO memory_privacy_state(user_id) VALUES($1) ON CONFLICT DO NOTHING",
        [user.into()],
    ))
    .await?;
    let row = tx.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "SELECT privacy_epoch,source_cutoff_at,clock_timestamp() AS observed_at FROM memory_privacy_state WHERE user_id=$1 FOR UPDATE", [user.into()])).await?.ok_or_else(|| anyhow::anyhow!("memory privacy state missing"))?;
    Ok(MemoryWriteContext {
        user_id: user,
        privacy_epoch: row.try_get("", "privacy_epoch")?,
        source_cutoff_at: row.try_get("", "source_cutoff_at")?,
        source_received_at: row.try_get("", "observed_at")?,
        source_event_ids: Vec::new(),
        source_memories: Vec::new(),
    })
}

pub(super) async fn validate(
    tx: &DatabaseTransaction,
    expected: &MemoryWriteContext,
) -> anyhow::Result<()> {
    let current = lock_user(tx, expected.user_id).await?;
    anyhow::ensure!(
        current.privacy_epoch == expected.privacy_epoch,
        "memory privacy epoch changed"
    );
    anyhow::ensure!(
        expected.source_received_at > current.source_cutoff_at,
        "memory source predates privacy boundary"
    );
    for id in &expected.source_event_ids {
        let valid = tx.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT m.id FROM chat_messages m JOIN chat_sessions s ON s.id=m.session_id WHERE m.id=$1 AND s.creator_id=$2 AND m.role='user' AND m.user_id=$2 AND m.source_received_at>$3 AND m.source_received_at<=$4",
            vec![(*id).into(),expected.user_id.to_string().into(),current.source_cutoff_at.into(),expected.source_received_at.into()])).await?.is_some();
        anyhow::ensure!(
            valid,
            "conversation source was erased, predates privacy boundary, or belongs to another user"
        );
    }
    for (id, revision) in &expected.source_memories {
        let valid = tx.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT id FROM memory_items WHERE id=$1::uuid AND user_id=$2 AND tenant_id=$3 AND revision=$4 AND status='active' AND (expires_at IS NULL OR expires_at>clock_timestamp())",
            vec![id.clone().into(), expected.user_id.into(), Uuid::nil().into(), (*revision).into()])).await?.is_some();
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
        user_id: user,
        privacy_epoch: 0,
        source_cutoff_at: DateTime::UNIX_EPOCH,
        source_received_at: Utc::now(),
        source_event_ids: Vec::new(),
        source_memories: Vec::new(),
    }
}
