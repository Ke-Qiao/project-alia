use std::time::Duration;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use uuid::Uuid;

/// The independent revocation domains captured at one trusted input admission.
/// Neither model callbacks nor queued consumers may refresh either field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceEpochs {
    pub privacy_epoch: i64,
    pub relationship_epoch: i64,
}

/// Admit a trusted source in the same database order used by memory erasure.
/// Capture once before queueing; timestamps remain audit data, not causal proof.
pub async fn capture_source_epoch(db: &DatabaseConnection, user: Uuid) -> anyhow::Result<i64> {
    Ok(capture_source_epochs(db, user).await?.privacy_epoch)
}

pub async fn capture_source_epochs(
    db: &DatabaseConnection,
    user: Uuid,
) -> anyhow::Result<SourceEpochs> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let tx = db.begin().await?;
        // Match authorization/cascade ordering: account before privacy state.
        let active = tx
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT id FROM users WHERE id=$1 AND status='active' FOR SHARE",
                [user.into()],
            ))
            .await?
            .is_some();
        anyhow::ensure!(
            active,
            "source admission requires a currently active account"
        );
        tx.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO memory_privacy_state(user_id) VALUES($1) ON CONFLICT DO NOTHING",
            [user.into()],
        ))
        .await?;
        let row = tx
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT privacy_epoch FROM memory_privacy_state WHERE user_id=$1 FOR UPDATE",
                [user.into()],
            ))
            .await?
            .ok_or_else(|| anyhow::anyhow!("memory privacy state missing"))?;
        let epoch: i64 = row.try_get("", "privacy_epoch")?;
        anyhow::ensure!(epoch >= 0, "invalid memory privacy epoch");
        let relationship_epoch = lock_relationship_epoch(&tx, user).await?;
        tx.commit().await?;
        Ok(SourceEpochs {
            privacy_epoch: epoch,
            relationship_epoch,
        })
    })
    .await
    .map_err(|_| anyhow::anyhow!("memory source admission timed out"))?
}

/// The caller must already hold the active owner and its privacy row. Shared
/// ordering is policy (when needed), account, privacy, then relationship.
pub(crate) async fn lock_relationship_epoch(
    tx: &sea_orm::DatabaseTransaction,
    user: Uuid,
) -> anyhow::Result<i64> {
    tx.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO relationship_authority(owner_id) VALUES($1) ON CONFLICT DO NOTHING",
        [user.into()],
    ))
    .await?;
    let row = tx
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT relationship_epoch FROM relationship_authority WHERE owner_id=$1 FOR UPDATE",
            [user.into()],
        ))
        .await?
        .ok_or_else(|| anyhow::anyhow!("relationship authority is missing"))?;
    let epoch = row.try_get("", "relationship_epoch")?;
    anyhow::ensure!(epoch >= 0, "invalid relationship epoch");
    Ok(epoch)
}
