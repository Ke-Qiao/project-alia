use std::{future::Future, time::Duration};

use alia_core::{
    body::{BodyCommand, BodyEnd, BodyProgress},
    delivery::VoiceScope,
};
use anyhow::{Context, Result, ensure};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait,
    Value as SqlValue,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{BodyDeliveryOrigin, BodyDeliveryRecord, BodyDispatch, BodyLookupError};
use crate::modules::delivery::repository::validate_response_source;

const ROW: &str = "SELECT to_jsonb(b) || jsonb_build_object('latest_checkpoint',(SELECT c.progress FROM body_delivery_late_checkpoints c WHERE c.delivery_id=b.delivery_id ORDER BY c.receipt_sequence DESC LIMIT 1)) AS record FROM body_deliveries b ";
const CAPACITY_LOCK: i64 = 0x414c_4941_424f_4459;

#[derive(Clone)]
pub struct BodyDeliveryRepository {
    db: DatabaseConnection,
}

impl BodyDeliveryRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Consult a known Response phase before even opening device control. This
    /// read keeps old facts available after erasure without granting new output.
    pub async fn lookup_origin(
        &self,
        origin: &BodyDeliveryOrigin,
        channel_id: &str,
        command: &BodyCommand,
    ) -> Result<Option<BodyDeliveryRecord>> {
        bounded(async {
            authorized_owner(&self.db, command.owner_id).await?;
            let old = record(&self.db, "JOIN delivery_responses r USING(response_id) WHERE b.response_id=$1 AND b.output_key=$2 AND r.user_id=$3", vec![origin.response_id.into(),origin.output_key.clone().into(),command.owner_id.into()]).await?;
            if let Some(old) = &old {
                ensure!(same_intent(&old.command, command), "body response phase was reused with different intent");
                ensure!(self.db.query_one(stmt("SELECT delivery_id FROM body_deliveries WHERE delivery_id=$1 AND channel_id=$2", vec![old.delivery_id.into(),channel_id.into()])).await?.is_some(), "body channel differs from original response");
            }
            Ok(old)
        }).await
    }

    /// The commit is the only grant to send this identity. Even if its ACK was
    /// lost, a second claim returns the original record, never a new permit.
    pub async fn claim(
        &self,
        origin: &BodyDeliveryOrigin,
        channel_id: &str,
        command: &BodyCommand,
    ) -> Result<BodyDispatch> {
        bounded(async {
            command.validate()?;
            ensure!(!origin.response_id.is_nil() && !origin.output_key.trim().is_empty() && origin.output_key.len() <= 256, "invalid body delivery origin");
            let command_value = serde_json::to_value(command)?;
            ensure!(serde_json::to_vec(&command_value)?.len() <= alia_core::body::MAX_BODY_PAYLOAD_BYTES, "body intent too large");
            let tx = self.db.begin().await?;
            let response = validate_response_source(&tx, command.owner_id, origin.response_id).await?;
            ensure!(response.kind != "source_notice", "fixed source notice cannot authorize body actions");
            ensure!(!response.cancel_requested && !matches!(response.status, crate::modules::delivery::types::ResponseStatus::Interrupted | crate::modules::delivery::types::ResponseStatus::Discarded), "response no longer authorizes body delivery");
            ensure!(response.channel_id == channel_id, "body channel differs from original response");
            let original_scope: Option<VoiceScope> = serde_json::from_value(response.content.get("voice_scope").cloned().unwrap_or(Value::Null))?;
            ensure!(original_scope == command.scope.voice(), "body scope differs from original response");
            if let Some(old) = record(&tx, "WHERE b.response_id=$1 AND b.output_key=$2 FOR UPDATE", vec![origin.response_id.into(), origin.output_key.clone().into()]).await? {
                ensure!(same_intent(&old.command, command), "body response phase was reused with different intent");
                tx.commit().await?;
                return Ok(BodyDispatch::Existing(old));
            }
            tx.execute(stmt("SELECT pg_advisory_xact_lock($1)", vec![CAPACITY_LOCK.into()])).await?;
            recover(&tx).await?;
            let row = tx.query_one(stmt("SELECT COUNT(*) FILTER (WHERE status IN ('sending','observing')) AS active, COUNT(*) FILTER (WHERE response_id=$1) AS response_count FROM body_deliveries", vec![origin.response_id.into()])).await?.context("body capacity unavailable")?;
            ensure!(row.try_get::<i64>("", "active")? < 64 && row.try_get::<i64>("", "response_count")? < 32, "body delivery capacity reached");
            let delivery_id = Uuid::new_v4();
            let dispatch_id = Uuid::new_v4();
            tx.execute(stmt("INSERT INTO body_deliveries(delivery_id,response_id,output_key,channel_id,source_command_id,dispatch_id,identity,command,status,lease_expires_at,observation_deadline_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'sending',clock_timestamp()+interval '15 seconds',clock_timestamp()+interval '45 seconds')", vec![delivery_id.into(), origin.response_id.into(), origin.output_key.clone().into(), channel_id.into(), command.source_command_id.clone().into(), dispatch_id.into(), serde_json::to_value(command.identity)?.into(), command_value.clone().into()])).await?;
            audit(&tx, origin.response_id, delivery_id, "body_dispatch", json!({"command":command_value})).await?;
            tx.commit().await?;
            Ok(BodyDispatch::Fresh { delivery_id, dispatch_id })
        }).await
    }

    /// Revalidate the original source under the same lock order as Memory
    /// erasure. Heartbeats never refresh the absolute observation deadline.
    pub async fn authorize(&self, delivery_id: Uuid, dispatch_id: Uuid) -> Result<bool> {
        bounded(async {
            let tx = self.db.begin().await?;
            let old = required(&tx, delivery_id, dispatch_id, false).await?;
            let response = validate_response_source(&tx, old.command.owner_id, old.response_id).await?;
            ensure!(response.kind != "source_notice", "fixed source notice cannot authorize body actions");
            let row = tx.query_one(stmt("UPDATE body_deliveries SET lease_expires_at=LEAST(clock_timestamp()+interval '15 seconds',observation_deadline_at),updated_at=clock_timestamp() WHERE delivery_id=$1 AND dispatch_id=$2 AND status IN ('sending','observing') AND lease_expires_at>clock_timestamp() AND observation_deadline_at>clock_timestamp() AND NOT $3 RETURNING delivery_id", vec![delivery_id.into(), dispatch_id.into(), (response.cancel_requested || matches!(response.status, crate::modules::delivery::types::ResponseStatus::Interrupted | crate::modules::delivery::types::ResponseStatus::Discarded)).into()])).await?;
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
        progress: &BodyProgress,
    ) -> Result<()> {
        bounded(async {
            let tx = self.db.begin().await?;
            let old = required(&tx, delivery_id, dispatch_id, true).await?;
            if old.progress.as_ref() == Some(progress) { tx.commit().await?; return Ok(()); }
            let mut previous = old.progress.clone();
            if old.status == "unknown" {
                let sequence = i64::try_from(progress.receipt_sequence).context("body receipt sequence overflow")?;
                if let Some(same) = tx.query_one(stmt("SELECT progress FROM body_delivery_late_checkpoints WHERE delivery_id=$1 AND receipt_sequence=$2", vec![delivery_id.into(), sequence.into()])).await? {
                    ensure!(same.try_get::<Value>("", "progress")? == serde_json::to_value(progress)?, "body checkpoint sequence reused with different evidence");
                    tx.commit().await?;
                    return Ok(());
                }
                if let Some(latest) = tx.query_one(stmt("SELECT progress FROM body_delivery_late_checkpoints WHERE delivery_id=$1 ORDER BY receipt_sequence DESC LIMIT 1", vec![delivery_id.into()])).await? {
                    previous = Some(serde_json::from_value(latest.try_get::<Value>("", "progress")?)?);
                }
                if let Some(previous) = &mut previous
                    && matches!(previous.end, Some(BodyEnd::Unknown { .. }))
                {
                    previous.end = None;
                }
            }
            progress.validate(previous.as_ref(), &old.command)?;
            let value = serde_json::to_value(progress)?;
            if old.status == "unknown" {
                let sequence = i64::try_from(progress.receipt_sequence).context("body receipt sequence overflow")?;
                let inserted = tx.execute(stmt("INSERT INTO body_delivery_late_checkpoints(delivery_id,receipt_sequence,progress) VALUES($1,$2,$3) ON CONFLICT DO NOTHING", vec![delivery_id.into(), sequence.into(), value.clone().into()])).await?.rows_affected();
                let checkpoint = tx.query_one(stmt("SELECT progress FROM body_delivery_late_checkpoints WHERE delivery_id=$1 AND receipt_sequence=$2", vec![delivery_id.into(), sequence.into()])).await?.context("late body checkpoint missing")?;
                ensure!(checkpoint.try_get::<Value>("", "progress")? == value, "body checkpoint sequence reused with different evidence");
                if inserted > 0 { audit(&tx, old.response_id, delivery_id, "body_late_checkpoint", json!({"progress":progress})).await?; }
            } else {
                ensure!(matches!(old.status.as_str(), "sending" | "observing"), "body delivery already terminal");
                let (status, reason) = match &progress.end {
                    Some(BodyEnd::Submitted) => ("submitted", None),
                    Some(BodyEnd::Cancelled { .. }) => ("cancelled", None),
                    Some(BodyEnd::Failed { .. }) => ("failed", None),
                    Some(BodyEnd::Rejected { .. }) => ("rejected", None),
                    Some(BodyEnd::Unknown { reason }) => ("unknown", Some(reason.clone())),
                    None => ("observing", None),
                };
                tx.execute(stmt("UPDATE body_deliveries SET status=$3,progress=$4,unknown_reason=$5,updated_at=clock_timestamp() WHERE delivery_id=$1 AND dispatch_id=$2", vec![delivery_id.into(), dispatch_id.into(), status.into(), value.into(), reason.into()])).await?;
                audit(&tx, old.response_id, delivery_id, "body_progress", json!({"progress":progress})).await?;
            }
            tx.commit().await?;
            Ok(())
        }).await
    }

    pub async fn unknown(&self, delivery_id: Uuid, dispatch_id: Uuid, reason: &str) -> Result<()> {
        bounded(async {
            ensure!(!reason.is_empty() && reason.len() <= 2048, "invalid body unknown reason");
            let tx = self.db.begin().await?;
            let old = required(&tx, delivery_id, dispatch_id, true).await?;
            if matches!(old.status.as_str(), "sending" | "observing") {
                tx.execute(stmt("UPDATE body_deliveries SET status='unknown',unknown_reason=$3,updated_at=clock_timestamp() WHERE delivery_id=$1 AND dispatch_id=$2", vec![delivery_id.into(), dispatch_id.into(), reason.into()])).await?;
                audit(&tx, old.response_id, delivery_id, "body_unknown", json!({"reason":reason})).await?;
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

    pub async fn get(&self, owner: Uuid, delivery_id: Uuid) -> Result<BodyDeliveryRecord> {
        bounded(async {
            authorized_owner(&self.db, owner).await?;
            record(&self.db, "JOIN delivery_responses r USING(response_id) WHERE b.delivery_id=$1 AND r.user_id=$2", vec![delivery_id.into(), owner.into()]).await?.ok_or_else(|| BodyLookupError::NotFound.into())
        }).await
    }

    pub async fn for_response(
        &self,
        owner: Uuid,
        response_id: Uuid,
    ) -> Result<Vec<BodyDeliveryRecord>> {
        bounded(async {
            authorized_owner(&self.db, owner).await?;
            if self.db.query_one(stmt("SELECT response_id FROM delivery_responses WHERE response_id=$1 AND user_id=$2", vec![response_id.into(), owner.into()])).await?.is_none() { return Err(BodyLookupError::NotFound.into()); }
            self.db.query_all(stmt(&format!("{ROW}JOIN delivery_responses r USING(response_id) WHERE b.response_id=$1 AND r.user_id=$2 ORDER BY b.created_at,b.delivery_id LIMIT 32"), vec![response_id.into(), owner.into()])).await?.into_iter().map(|row| Ok(serde_json::from_value(row.try_get::<Value>("", "record")?)?)).collect()
        }).await
    }
}

async fn bounded<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .context("body database confirmation timed out; committed state may be unknown")?
}

fn same_intent(old: &BodyCommand, new: &BodyCommand) -> bool {
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
) -> Result<BodyDeliveryRecord> {
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
    .context("body dispatch not found")
}

async fn record(
    db: &impl ConnectionTrait,
    suffix: &str,
    values: Vec<SqlValue>,
) -> Result<Option<BodyDeliveryRecord>> {
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
        return Err(BodyLookupError::Unauthorized.into());
    }
    Ok(())
}

async fn recover(db: &impl ConnectionTrait) -> Result<()> {
    db.execute_unprepared("WITH expired AS (UPDATE body_deliveries SET status='unknown',unknown_reason='body observation owner expired',updated_at=clock_timestamp() WHERE status IN ('sending','observing') AND (lease_expires_at<=clock_timestamp() OR observation_deadline_at<=clock_timestamp()) RETURNING delivery_id,response_id) INSERT INTO delivery_events(response_id,kind,payload) SELECT response_id,'body_unknown',jsonb_build_object('body_delivery_id',delivery_id,'reason','body observation owner expired') FROM expired").await?;
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
                    fields.insert("body_delivery_id".into(), json!(delivery));
                    Value::Object(fields)
                })
                .context("body audit must be an object")?)
            .into(),
        ],
    ))
    .await?;
    Ok(())
}
fn stmt(sql: &str, values: Vec<SqlValue>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}
