use std::{future::Future, time::Duration};

use alia_core::{
    delivery::VoiceScope,
    embodiment::motion::{MotionCommand, MotionEnd, MotionProgress},
};
use anyhow::{Context, Result, ensure};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait,
    Value as SqlValue,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{MotionDeliveryOrigin, MotionDeliveryRecord, MotionDispatch, MotionLookupError};
use crate::modules::delivery::repository::validate_response_source;

const ROW: &str = "SELECT to_jsonb(b) || jsonb_build_object('latest_checkpoint',(SELECT c.progress FROM motion_delivery_late_checkpoints c WHERE c.delivery_id=b.delivery_id ORDER BY c.receipt_sequence DESC LIMIT 1)) AS record FROM motion_deliveries b ";
const CAPACITY_LOCK: i64 = 0x414c_4941_424f_4459;

#[derive(Clone)]
pub struct MotionDeliveryRepository {
    db: DatabaseConnection,
}

impl MotionDeliveryRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Consult a known Response phase before even opening device control. This
    /// read keeps old facts available after erasure without granting new output.
    /// The caller must compare its original intent to the returned command;
    /// lookup cannot resolve historical asset pins from today's catalog.
    pub async fn lookup_origin(
        &self,
        origin: &MotionDeliveryOrigin,
        channel_id: &str,
        owner_id: Uuid,
    ) -> Result<Option<MotionDeliveryRecord>> {
        bounded(async {
            authorized_owner(&self.db, owner_id).await?;
            let old = record(&self.db, "JOIN delivery_responses r USING(response_id) WHERE b.response_id=$1 AND b.output_key=$2 AND r.user_id=$3", vec![origin.response_id.into(),origin.output_key.clone().into(),owner_id.into()]).await?;
            if let Some(old) = &old {
                ensure!(old.command.owner_id == owner_id, "motion record differs from original owner");
                ensure!(self.db.query_one(stmt("SELECT delivery_id FROM motion_deliveries WHERE delivery_id=$1 AND channel_id=$2", vec![old.delivery_id.into(),channel_id.into()])).await?.is_some(), "motion channel differs from original response");
            }
            Ok(old)
        }).await
    }

    /// The commit is the only grant to send this identity. Even if its ACK was
    /// lost, a second claim returns the original record, never a new permit.
    pub async fn claim(
        &self,
        origin: &MotionDeliveryOrigin,
        channel_id: &str,
        command: &MotionCommand,
    ) -> Result<MotionDispatch> {
        bounded(async {
            command.validate()?;
            ensure!(!origin.response_id.is_nil() && !origin.output_key.trim().is_empty() && origin.output_key.len() <= 256, "invalid motion delivery origin");
            let command_value = serde_json::to_value(command)?;
            ensure!(serde_json::to_vec(&command_value)?.len() <= alia_core::embodiment::motion::MAX_MOTION_PAYLOAD_BYTES, "motion intent too large");
            let tx = self.db.begin().await?;
            let response = validate_response_source(&tx, command.owner_id, origin.response_id).await?;
            ensure!(response.kind != "source_notice", "fixed source notice cannot authorize motion actions");
            ensure!(!response.cancel_requested && !matches!(response.status, crate::modules::delivery::types::ResponseStatus::Interrupted | crate::modules::delivery::types::ResponseStatus::Discarded), "response no longer authorizes motion delivery");
            ensure!(response.channel_id == channel_id, "motion channel differs from original response");
            let original_scope: Option<VoiceScope> = serde_json::from_value(response.content.get("voice_scope").cloned().unwrap_or(Value::Null))?;
            ensure!(original_scope == command.scope.voice(), "motion scope differs from original response");
            if let Some(old) = record(&tx, "WHERE b.response_id=$1 AND b.output_key=$2 FOR UPDATE", vec![origin.response_id.into(), origin.output_key.clone().into()]).await? {
                ensure!(same_intent(&old.command, command), "motion response phase was reused with different intent");
                tx.commit().await?;
                return Ok(MotionDispatch::Existing(old));
            }
            tx.execute(stmt("SELECT pg_advisory_xact_lock($1)", vec![CAPACITY_LOCK.into()])).await?;
            recover(&tx).await?;
            let row = tx.query_one(stmt("SELECT COUNT(*) FILTER (WHERE status IN ('sending','observing')) AS active, COUNT(*) FILTER (WHERE response_id=$1) AS response_count FROM motion_deliveries", vec![origin.response_id.into()])).await?.context("motion capacity unavailable")?;
            ensure!(row.try_get::<i64>("", "active")? < 64 && row.try_get::<i64>("", "response_count")? < 32, "motion delivery capacity reached");
            let delivery_id = Uuid::new_v4();
            let dispatch_id = Uuid::new_v4();
            tx.execute(stmt("INSERT INTO motion_deliveries(delivery_id,response_id,output_key,channel_id,source_command_id,dispatch_id,identity,command,status,lease_expires_at,observation_deadline_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'sending',clock_timestamp()+interval '15 seconds',clock_timestamp()+interval '45 seconds')", vec![delivery_id.into(), origin.response_id.into(), origin.output_key.clone().into(), channel_id.into(), command.source_command_id.clone().into(), dispatch_id.into(), serde_json::to_value(command.identity)?.into(), command_value.clone().into()])).await?;
            audit(&tx, origin.response_id, delivery_id, "motion_dispatch", json!({"command":command_value})).await?;
            tx.commit().await?;
            Ok(MotionDispatch::Fresh { delivery_id, dispatch_id })
        }).await
    }

    /// Revalidate the original source under the same lock order as Memory
    /// erasure. Heartbeats never refresh the absolute observation deadline.
    pub async fn authorize(&self, delivery_id: Uuid, dispatch_id: Uuid) -> Result<bool> {
        bounded(async {
            let tx = self.db.begin().await?;
            let old = required(&tx, delivery_id, dispatch_id, false).await?;
            let response = validate_response_source(&tx, old.command.owner_id, old.response_id).await?;
            ensure!(response.kind != "source_notice", "fixed source notice cannot authorize motion actions");
            let row = tx.query_one(stmt("UPDATE motion_deliveries SET lease_expires_at=LEAST(clock_timestamp()+interval '15 seconds',observation_deadline_at),updated_at=clock_timestamp() WHERE delivery_id=$1 AND dispatch_id=$2 AND status IN ('sending','observing') AND lease_expires_at>clock_timestamp() AND observation_deadline_at>clock_timestamp() AND NOT $3 RETURNING delivery_id", vec![delivery_id.into(), dispatch_id.into(), (response.cancel_requested || matches!(response.status, crate::modules::delivery::types::ResponseStatus::Interrupted | crate::modules::delivery::types::ResponseStatus::Discarded)).into()])).await?;
            tx.commit().await?;
            Ok(row.is_some())
        }).await
    }

    /// Device observations remain facts even after policy/owner revocation.
    /// A local Unknown stays frozen; later evidence has an independent audit.
    pub async fn progress(
        &self,
        delivery_id: Uuid,
        dispatch_id: Uuid,
        progress: &MotionProgress,
    ) -> Result<()> {
        bounded(async {
            let tx = self.db.begin().await?;
            let old = required(&tx, delivery_id, dispatch_id, true).await?;
            if old.progress.as_ref() == Some(progress) { tx.commit().await?; return Ok(()); }
            let mut previous = old.progress.clone();
            if old.status == "unknown" {
                let sequence = i64::try_from(progress.receipt_sequence).context("motion receipt sequence overflow")?;
                if let Some(same) = tx.query_one(stmt("SELECT progress FROM motion_delivery_late_checkpoints WHERE delivery_id=$1 AND receipt_sequence=$2", vec![delivery_id.into(), sequence.into()])).await? {
                    ensure!(same.try_get::<Value>("", "progress")? == serde_json::to_value(progress)?, "motion checkpoint sequence reused with different evidence");
                    tx.commit().await?;
                    return Ok(());
                }
                if let Some(latest) = tx.query_one(stmt("SELECT progress FROM motion_delivery_late_checkpoints WHERE delivery_id=$1 ORDER BY receipt_sequence DESC LIMIT 1", vec![delivery_id.into()])).await? {
                    previous = Some(serde_json::from_value(latest.try_get::<Value>("", "progress")?)?);
                }
                if let Some(previous) = &mut previous
                    && matches!(previous.end, Some(MotionEnd::Unknown { .. }))
                {
                    previous.end = None;
                }
            }
            progress.validate(previous.as_ref(), &old.command)?;
            let value = serde_json::to_value(progress)?;
            if old.status == "unknown" {
                let sequence = i64::try_from(progress.receipt_sequence).context("motion receipt sequence overflow")?;
                let inserted = tx.execute(stmt("INSERT INTO motion_delivery_late_checkpoints(delivery_id,receipt_sequence,progress) VALUES($1,$2,$3) ON CONFLICT DO NOTHING", vec![delivery_id.into(), sequence.into(), value.clone().into()])).await?.rows_affected();
                let checkpoint = tx.query_one(stmt("SELECT progress FROM motion_delivery_late_checkpoints WHERE delivery_id=$1 AND receipt_sequence=$2", vec![delivery_id.into(), sequence.into()])).await?.context("late motion checkpoint missing")?;
                ensure!(checkpoint.try_get::<Value>("", "progress")? == value, "motion checkpoint sequence reused with different evidence");
                if inserted > 0 { audit(&tx, old.response_id, delivery_id, "motion_late_checkpoint", json!({"progress":progress})).await?; }
            } else {
                ensure!(matches!(old.status.as_str(), "sending" | "observing"), "motion delivery already terminal");
                let (status, reason) = match &progress.end {
                    Some(MotionEnd::Submitted) => ("submitted", None),
                    Some(MotionEnd::Cancelled { .. }) => ("cancelled", None),
                    Some(MotionEnd::Failed { .. }) => ("failed", None),
                    Some(MotionEnd::Rejected { .. }) => ("rejected", None),
                    Some(MotionEnd::Unknown { reason }) => ("unknown", Some(reason.clone())),
                    None => ("observing", None),
                };
                tx.execute(stmt("UPDATE motion_deliveries SET status=$3,progress=$4,unknown_reason=$5,updated_at=clock_timestamp() WHERE delivery_id=$1 AND dispatch_id=$2", vec![delivery_id.into(), dispatch_id.into(), status.into(), value.into(), reason.into()])).await?;
                audit(&tx, old.response_id, delivery_id, "motion_progress", json!({"progress":progress})).await?;
            }
            tx.commit().await?;
            Ok(())
        }).await
    }

    pub async fn unknown(&self, delivery_id: Uuid, dispatch_id: Uuid, reason: &str) -> Result<()> {
        bounded(async {
            ensure!(!reason.is_empty() && reason.len() <= 2048, "invalid motion unknown reason");
            let tx = self.db.begin().await?;
            let old = required(&tx, delivery_id, dispatch_id, true).await?;
            if matches!(old.status.as_str(), "sending" | "observing") {
                tx.execute(stmt("UPDATE motion_deliveries SET status='unknown',unknown_reason=$3,updated_at=clock_timestamp() WHERE delivery_id=$1 AND dispatch_id=$2", vec![delivery_id.into(), dispatch_id.into(), reason.into()])).await?;
                audit(&tx, old.response_id, delivery_id, "motion_unknown", json!({"reason":reason})).await?;
            }
            tx.commit().await?;
            Ok(())
        }).await
    }

    pub async fn recover_expired(&self) -> Result<()> {
        bounded(async {
            let tx = self.db.begin().await?;
            recover(&tx).await?;
            tx.commit().await?;
            Ok(())
        })
        .await
    }

    pub async fn get(&self, owner: Uuid, delivery_id: Uuid) -> Result<MotionDeliveryRecord> {
        bounded(async {
            authorized_owner(&self.db, owner).await?;
            record(&self.db, "JOIN delivery_responses r USING(response_id) WHERE b.delivery_id=$1 AND r.user_id=$2", vec![delivery_id.into(), owner.into()]).await?.ok_or_else(|| MotionLookupError::NotFound.into())
        }).await
    }

    pub async fn for_response(
        &self,
        owner: Uuid,
        response_id: Uuid,
    ) -> Result<Vec<MotionDeliveryRecord>> {
        bounded(async {
            authorized_owner(&self.db, owner).await?;
            if self.db.query_one(stmt("SELECT response_id FROM delivery_responses WHERE response_id=$1 AND user_id=$2", vec![response_id.into(), owner.into()])).await?.is_none() { return Err(MotionLookupError::NotFound.into()); }
            self.db.query_all(stmt(&format!("{ROW}JOIN delivery_responses r USING(response_id) WHERE b.response_id=$1 AND r.user_id=$2 ORDER BY b.created_at,b.delivery_id LIMIT 32"), vec![response_id.into(), owner.into()])).await?.into_iter().map(|row| Ok(serde_json::from_value(row.try_get::<Value>("", "record")?)?)).collect()
        }).await
    }
}

async fn bounded<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .context("motion database confirmation timed out; committed state may be unknown")?
}

fn same_intent(old: &MotionCommand, new: &MotionCommand) -> bool {
    // Control identity and caller correlation are allocations. Response+phase
    // owns the semantic plan; restart cannot create a new dispatch permission.
    old.plan_id == new.plan_id
        && old.owner_id == new.owner_id
        && old.scope == new.scope
        && old.priority == new.priority
        && old.interruptible == new.interruptible
        && old.actions == new.actions
}

async fn required(
    db: &impl ConnectionTrait,
    id: Uuid,
    dispatch: Uuid,
    lock: bool,
) -> Result<MotionDeliveryRecord> {
    record(
        db,
        if lock {
            "WHERE b.delivery_id=$1 AND b.dispatch_id=$2 FOR UPDATE"
        } else {
            "WHERE b.delivery_id=$1 AND b.dispatch_id=$2"
        },
        vec![id.into(), dispatch.into()],
    )
    .await?
    .context("motion dispatch not found")
}

async fn record(
    db: &impl ConnectionTrait,
    suffix: &str,
    values: Vec<SqlValue>,
) -> Result<Option<MotionDeliveryRecord>> {
    db.query_one(stmt(&format!("{ROW}{suffix}"), values))
        .await?
        .map(|row| Ok(serde_json::from_value(row.try_get::<Value>("", "record")?)?))
        .transpose()
}

async fn authorized_owner(db: &impl ConnectionTrait, owner: Uuid) -> Result<()> {
    if db
        .query_one(stmt(
            "SELECT id FROM users WHERE id=$1 AND status='active'",
            vec![owner.into()],
        ))
        .await?
        .is_none()
    {
        return Err(MotionLookupError::Unauthorized.into());
    }
    Ok(())
}

async fn recover(db: &impl ConnectionTrait) -> Result<()> {
    db.execute_unprepared("WITH expired AS (UPDATE motion_deliveries SET status='unknown',unknown_reason='motion observation owner expired',updated_at=clock_timestamp() WHERE status IN ('sending','observing') AND (lease_expires_at<=clock_timestamp() OR observation_deadline_at<=clock_timestamp()) RETURNING delivery_id,response_id) INSERT INTO delivery_events(response_id,kind,payload) SELECT response_id,'motion_unknown',jsonb_build_object('motion_delivery_id',delivery_id,'reason','motion observation owner expired') FROM expired").await?;
    Ok(())
}

async fn audit(
    db: &impl ConnectionTrait,
    response: Uuid,
    delivery: Uuid,
    kind: &str,
    payload: Value,
) -> Result<()> {
    db.execute(stmt(
        "INSERT INTO delivery_events(response_id,kind,payload) VALUES($1,$2,$3)",
        vec![
            response.into(),
            kind.into(),
            (payload
                .as_object()
                .map(|fields| {
                    let mut fields = fields.clone();
                    fields.insert("motion_delivery_id".into(), json!(delivery));
                    Value::Object(fields)
                })
                .context("motion audit must be an object")?)
            .into(),
        ],
    ))
    .await?;
    Ok(())
}
fn stmt(sql: &str, values: Vec<SqlValue>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}
