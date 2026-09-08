use std::time::Duration;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use uuid::Uuid;

/// Admit a trusted source in the same database order used by memory erasure.
/// Capture once before queueing; timestamps remain audit data, not causal proof.
pub async fn capture_source_epoch(db: &DatabaseConnection, user: Uuid) -> anyhow::Result<i64> {
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
        tx.commit().await?;
        Ok(epoch)
    })
    .await
    .map_err(|_| anyhow::anyhow!("memory source admission timed out"))?
}
