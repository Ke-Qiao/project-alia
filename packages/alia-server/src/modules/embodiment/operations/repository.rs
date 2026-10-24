use anyhow::{Context, Result, ensure};
use sea_orm::sqlx::{Connection, PgConnection};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, Statement,
    TransactionTrait, Value as SqlValue,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{AdapterDescriptor, OperationFeedback, OperationRecord, OperationStatus};
use crate::providers::adapter::ToolContext;

const SELECT: &str = "SELECT to_jsonb(o) AS record FROM embodiment_operations o ";

#[derive(Debug, thiserror::Error)]
#[error("feedback sequence is not newer")]
pub(super) struct SupersededFeedback;

/// A dedicated session avoids starving the regular pool while a device call
/// remains outstanding. Dropping this owner closes the connection, never puts a
/// session-level advisory lock back into the shared pool.
pub(super) struct CallLease {
    _connection: PgConnection,
}

#[derive(Clone)]
pub(super) struct Repository(pub DatabaseConnection);

fn sql(query: &str, values: Vec<SqlValue>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}

async fn load(
    db: &impl ConnectionTrait,
    suffix: &str,
    values: Vec<SqlValue>,
) -> Result<Option<OperationRecord>> {
    db.query_one(sql(&format!("{SELECT}{suffix}"), values))
        .await?
        .map(|row| Ok(serde_json::from_value(row.try_get::<Value>("", "record")?)?))
        .transpose()
}

async fn active(db: &impl ConnectionTrait, owner: Uuid) -> Result<()> {
    ensure!(
        db.query_one(sql(
            "SELECT id FROM users WHERE id=$1 AND status='active' FOR SHARE",
            vec![owner.into()]
        ))
        .await?
        .is_some(),
        "operation owner is unavailable"
    );
    Ok(())
}

/// The row lock binds the mutation to an existing, currently dispatched effect.
/// A ToolContext fabricated from request JSON cannot manufacture this evidence.
async fn effect(
    tx: &DatabaseTransaction,
    ctx: &ToolContext,
    name: &str,
    args: &Value,
) -> Result<Uuid> {
    active(tx, ctx.user_id).await?;
    let row = tx.query_one(sql(
        "SELECT effect_id FROM execution_effects WHERE user_id=$1 AND request_id=$2 AND invocation_key=$3 AND tool_name=$4 AND arguments=$5 AND status='dispatched' AND claim_id IS NOT NULL AND deadline_at>clock_timestamp() AND context_id IS NOT DISTINCT FROM $6 AND source=$7 AND source_event_id IS NOT DISTINCT FROM $8 AND source_privacy_epoch IS NOT DISTINCT FROM $9 AND conversation_id IS NOT DISTINCT FROM $10 AND channel_id IS NOT DISTINCT FROM $11 AND source_received_at=$12 FOR UPDATE",
        vec![ctx.user_id.into(),ctx.request_id.into(),ctx.invocation_key.clone().into(),name.into(),args.clone().into(),ctx.context_id.into(),ctx.source.clone().into(),ctx.source_event_id.into(),ctx.source_privacy_epoch.into(),ctx.conversation_id.into(),ctx.channel_id.clone().into(),ctx.source_received_at.into()],
    )).await?.context("operation requires its exact, live Dispatched Execution effect")?;
    Ok(row.try_get("", "effect_id")?)
}

async fn event(
    tx: &DatabaseTransaction,
    id: Uuid,
    status: OperationStatus,
    evidence: Value,
) -> Result<()> {
    let status = serde_json::to_value(status)?
        .as_str()
        .context("status string")?
        .to_owned();
    tx.execute(sql(
        "INSERT INTO embodiment_operation_events(operation_id,status,evidence) VALUES($1,$2,$3)",
        vec![id.into(), status.into(), evidence.into()],
    ))
    .await?;
    tx.execute(sql("DELETE FROM embodiment_operation_events WHERE operation_id=$1 AND event_id NOT IN (SELECT event_id FROM embodiment_operation_events WHERE operation_id=$1 ORDER BY event_id DESC LIMIT 64)",vec![id.into()])).await?;
    Ok(())
}

impl Repository {
    pub async fn lease(&self, id: Uuid, wait: bool) -> Result<Option<CallLease>> {
        let options = self.0.get_postgres_connection_pool().connect_options();
        let mut connection = PgConnection::connect_with(&options).await?;
        let acquired = if wait {
            sea_orm::sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 179783))")
                .bind(id.to_string())
                .execute(&mut connection)
                .await?;
            true
        } else {
            sea_orm::sqlx::query_scalar::<_, bool>(
                "SELECT pg_try_advisory_lock(hashtextextended($1, 179783))",
            )
            .bind(id.to_string())
            .fetch_one(&mut connection)
            .await?
        };
        Ok(acquired.then_some(CallLease {
            _connection: connection,
        }))
    }

    pub async fn current(&self, id: Uuid) -> Result<OperationRecord> {
        load(&self.0, "WHERE o.operation_id=$1", vec![id.into()])
            .await?
            .context("operation missing")
    }
    pub async fn get(&self, owner: Uuid, id: Uuid) -> Result<Option<OperationRecord>> {
        let tx = self.0.begin().await?;
        active(&tx, owner).await?;
        let result = load(
            &tx,
            "WHERE o.owner_id=$1 AND o.operation_id=$2",
            vec![owner.into(), id.into()],
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn list(&self, owner: Uuid, limit: u64) -> Result<Vec<OperationRecord>> {
        let tx = self.0.begin().await?;
        active(&tx, owner).await?;
        let rows=tx.query_all(sql(&format!("{SELECT}WHERE o.owner_id=$1 ORDER BY o.created_at_ms DESC,o.operation_id DESC LIMIT $2"),vec![owner.into(),(limit.clamp(1,100) as i64).into()])).await?;
        let result = rows
            .into_iter()
            .map(|row| Ok(serde_json::from_value(row.try_get::<Value>("", "record")?)?))
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn create(
        &self,
        binding: &AdapterDescriptor,
        ctx: &ToolContext,
        args: &Value,
    ) -> Result<(OperationRecord, Option<CallLease>)> {
        ensure!(
            ctx.user_id == binding.owner_id,
            "device grant belongs to a different principal"
        );
        ensure!(
            serde_json::to_vec(args)?.len() <= 24 * 1024,
            "operation goal exceeds byte budget"
        );
        let tx = self.0.begin().await?;
        let effect_id = effect(&tx, ctx, &binding.start_tool_name(), args).await?;
        if let Some(existing) =
            load(&tx, "WHERE o.start_effect_id=$1", vec![effect_id.into()]).await?
        {
            ensure!(
                binding.matches(&existing.identity),
                "operation implementation binding changed"
            );
            tx.commit().await?;
            return Ok((existing, None));
        }
        // Serialize resource acquisition in a deterministic order across processes.
        let id = Uuid::new_v4();
        tx.execute(sql("INSERT INTO embodiment_operations(operation_id,owner_id,start_effect_id,identity,goal,resources,status,next_observe_at) VALUES($1,$2,$3,$4,$5,$6,'submitting',clock_timestamp()+interval '5 seconds')",vec![id.into(),ctx.user_id.into(),effect_id.into(),serde_json::to_value(binding.identity(id))?.into(),args.clone().into(),serde_json::to_value(&binding.resources)?.into()])).await?;
        let mut resources = binding.resources.clone();
        resources.sort();
        for resource in resources {
            let result=tx.execute(sql("INSERT INTO embodiment_operation_resources(device_id,resource,operation_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",vec![binding.device_id.into(),resource.into(),id.into()])).await?;
            ensure!(
                result.rows_affected() == 1,
                "device resource is already owned by another operation"
            );
        }
        event(&tx,id,OperationStatus::Submitting,json!({"start_effect_id":effect_id,"meaning":"persisted for submission; not completion"})).await?;
        let record = load(&tx, "WHERE o.operation_id=$1", vec![id.into()])
            .await?
            .context("inserted operation missing")?;
        // Own external-call ordering before the ID becomes visible to cancel
        // or a recovering service. This new ID cannot already be locked.
        let lease = self
            .lease(id, true)
            .await?
            .context("new operation call lease")?;
        tx.commit().await?;
        Ok((record, Some(lease)))
    }

    pub async fn cancel(
        &self,
        binding: &AdapterDescriptor,
        ctx: &ToolContext,
        args: &Value,
        id: Uuid,
    ) -> Result<(OperationRecord, bool)> {
        ensure!(
            ctx.user_id == binding.owner_id,
            "device grant belongs to a different principal"
        );
        let tx = self.0.begin().await?;
        let effect_id = effect(&tx, ctx, &binding.cancel_tool_name(), args).await?;
        let record = load(
            &tx,
            "WHERE o.operation_id=$1 AND o.owner_id=$2 FOR UPDATE OF o",
            vec![id.into(), ctx.user_id.into()],
        )
        .await?
        .context("operation not found")?;
        ensure!(
            binding.matches(&record.identity),
            "operation device boot or implementation binding changed"
        );
        let inserted=tx.execute(sql("INSERT INTO embodiment_operation_cancellations(effect_id,operation_id) VALUES($1,$2) ON CONFLICT DO NOTHING",vec![effect_id.into(),id.into()])).await?.rows_affected()==1;
        if inserted {
            tx.execute(sql(
                "UPDATE embodiment_operations SET cancel_effect_id=$2 WHERE operation_id=$1",
                vec![id.into(), effect_id.into()],
            ))
            .await?;
        }
        let dispatch = inserted && !(record.status.is_terminal() && record.stopped.is_some());
        if dispatch {
            let status = if record.status.is_terminal() {
                record.status
            } else {
                OperationStatus::Cancelling
            };
            tx.execute(sql("UPDATE embodiment_operations SET status=$2,updated_at_ms=(extract(epoch FROM clock_timestamp())*1000)::bigint WHERE operation_id=$1",vec![id.into(),serde_json::to_value(status)?.as_str().context("status")?.to_owned().into()])).await?;
            event(
                &tx,
                id,
                status,
                json!({"cancel_effect_id":effect_id,"stopped":false}),
            )
            .await?;
        }
        let record = load(&tx, "WHERE o.operation_id=$1", vec![id.into()])
            .await?
            .context("operation disappeared")?;
        tx.commit().await?;
        Ok((record, dispatch))
    }

    /// Claim a bounded query batch. Recovery never calls start or cancel.
    pub async fn due(&self) -> Result<Vec<OperationRecord>> {
        let rows=self.0.query_all(sql(
            "UPDATE embodiment_operations o SET next_observe_at=clock_timestamp()+interval '2 seconds' WHERE o.operation_id IN (SELECT operation_id FROM embodiment_operations WHERE next_observe_at<=clock_timestamp() AND (status NOT IN ('succeeded','failed','cancelled') OR stopped IS NULL) ORDER BY next_observe_at,operation_id LIMIT 8 FOR UPDATE SKIP LOCKED) RETURNING to_jsonb(o) AS record",vec![])).await?;
        rows.into_iter()
            .map(|row| Ok(serde_json::from_value(row.try_get::<Value>("", "record")?)?))
            .collect()
    }

    pub async fn unknown(&self, id: Uuid, reason: &str) -> Result<()> {
        let tx = self.0.begin().await?;
        let record = load(
            &tx,
            "WHERE o.operation_id=$1 FOR UPDATE OF o",
            vec![id.into()],
        )
        .await?
        .context("operation missing")?;
        let reason: String = reason.chars().take(512).collect();
        // Preserve proven completion. A later failed read cannot undo it.
        let status = if record.status.is_terminal() {
            record.status
        } else {
            OperationStatus::Unknown
        };
        tx.execute(sql("UPDATE embodiment_operations SET status=$2,uncertainty=$3,updated_at_ms=(extract(epoch FROM clock_timestamp())*1000)::bigint WHERE operation_id=$1",vec![id.into(),serde_json::to_value(status)?.as_str().context("status")?.to_owned().into(),reason.clone().into()])).await?;
        event(&tx, id, status, json!({"uncertainty":reason})).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn feedback(&self, feedback: OperationFeedback) -> Result<()> {
        ensure!(
            serde_json::to_vec(&feedback)?.len() <= 12 * 1024,
            "complete feedback budget exceeded"
        );
        ensure!(
            serde_json::to_vec(&feedback.payload)?.len() <= 12 * 1024,
            "feedback payload budget exceeded"
        );
        ensure!(
            serde_json::to_vec(&feedback.stopped)?.len() <= 12 * 1024,
            "stop evidence budget exceeded"
        );
        ensure!(
            feedback.sequence > 0 && feedback.sequence <= i64::MAX as u64,
            "invalid feedback sequence"
        );
        let tx = self.0.begin().await?;
        let id = feedback.identity.operation_id;
        let record = load(
            &tx,
            "WHERE o.operation_id=$1 FOR UPDATE OF o",
            vec![id.into()],
        )
        .await?
        .context("operation missing")?;
        ensure!(
            record.identity == feedback.identity,
            "feedback device boot or implementation identity mismatch"
        );
        let now: i64 = tx
            .query_one(sql(
                "SELECT (extract(epoch FROM clock_timestamp())*1000)::bigint AS now",
                vec![],
            ))
            .await?
            .context("database clock")?
            .try_get("", "now")?;
        if feedback.sequence <= record.last_sequence {
            return Err(SupersededFeedback.into());
        }
        ensure!(
            feedback.observed_at_ms >= now - 30_000 && feedback.observed_at_ms <= now + 2_000,
            "feedback is stale or from the future"
        );
        ensure!(
            record
                .last_feedback_at_ms
                .is_none_or(|last| feedback.observed_at_ms >= last),
            "feedback time regressed"
        );
        ensure!(
            feedback.status != OperationStatus::Submitting,
            "adapter cannot report host submission state"
        );
        // A queued host cancellation does not erase an earlier start's eventual
        // acknowledgement. Terminal outcomes also survive a stop request ACK.
        let cancellation_progress = (record.cancel_effect_id.is_some()
            && !record.status.is_terminal()
            && matches!(
                feedback.status,
                OperationStatus::Accepted | OperationStatus::Running
            ))
            || (record.status.is_terminal() && feedback.status == OperationStatus::Cancelling);
        let status = if cancellation_progress {
            if record.status.is_terminal() {
                record.status
            } else {
                OperationStatus::Cancelling
            }
        } else {
            feedback.status
        };
        ensure!(
            record.status.allows(status),
            "illegal operation state transition"
        );
        ensure!(
            record
                .last_confirmed_status
                .is_none_or(|last| last.allows(feedback.status)
                    || (last.is_terminal() && feedback.status == OperationStatus::Cancelling)),
            "feedback regressed last confirmed state"
        );
        if feedback.status == OperationStatus::Cancelling {
            ensure!(tx.query_one(sql("SELECT effect_id FROM embodiment_operation_cancellations WHERE operation_id=$1 LIMIT 1",vec![id.into()])).await?.is_some(),"cancelling feedback has no authorized cancellation effect");
        }
        if let Some(stopped) = &feedback.stopped {
            ensure!(
                stopped.observed_at_ms >= record.created_at_ms
                    && stopped.observed_at_ms <= feedback.observed_at_ms
                    && stopped.observed_at_ms >= now - 30_000,
                "invalid stop evidence time"
            );
            ensure!(!stopped.evidence.is_null(), "stop evidence is required");
            ensure!(
                feedback.status.is_terminal(),
                "stop evidence requires a terminal observation"
            );
        }
        let stopped = feedback.stopped.clone().or(record.stopped);
        let confirmed = if feedback.status == OperationStatus::Unknown
            || (record.status.is_terminal() && feedback.status == OperationStatus::Cancelling)
        {
            record.last_confirmed_status
        } else {
            Some(feedback.status)
        };
        tx.execute(sql("UPDATE embodiment_operations SET status=$2,last_confirmed_status=$9,last_sequence=$3,last_feedback_at_ms=$4,feedback=$5,stopped=$6,uncertainty=$7,updated_at_ms=$8 WHERE operation_id=$1",vec![id.into(),serde_json::to_value(status)?.as_str().context("status")?.to_owned().into(),(feedback.sequence as i64).into(),feedback.observed_at_ms.into(),feedback.payload.clone().into(),stopped.as_ref().map(serde_json::to_value).transpose()?.into(),(feedback.status==OperationStatus::Unknown).then(||"adapter reported unknown".to_string()).into(),now.into(),confirmed.map(|s|serde_json::to_value(s).expect("operation enum").as_str().expect("operation enum string").to_owned()).into()])).await?;
        event(&tx, id, status, serde_json::to_value(&feedback)?).await?;
        if status.is_terminal() && stopped.is_some() {
            tx.execute(sql(
                "DELETE FROM embodiment_operation_resources WHERE operation_id=$1",
                vec![id.into()],
            ))
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
