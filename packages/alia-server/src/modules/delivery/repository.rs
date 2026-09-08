//! Every state change and its audit event commit in one PostgreSQL transaction.
//! There is intentionally no dispatch outbox: recovering an owner never replays audio.
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, Statement,
    TransactionTrait, Value as SqlValue,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::types::*;

const RESPONSE: &str = "SELECT to_jsonb(r) || jsonb_build_object('key', jsonb_build_object('user_id',r.user_id,'request_id',r.request_id,'output_key',r.output_key)) AS record FROM delivery_responses r ";
const DELIVERY: &str = "SELECT to_jsonb(d) AS record FROM deliveries d ";
const CAPACITY_LOCK: i64 = 0x414c_4941_4445_4c49;

#[derive(Clone)]
pub struct DeliveryRepository {
    db: DatabaseConnection,
}

impl DeliveryRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn prepare(&self, input: NewResponse) -> DeliveryResult<ResponseRecord> {
        let tx = self.db.begin().await?;
        let response = self.prepare_in_transaction(&tx, input).await?;
        tx.commit().await?;
        Ok(response)
    }

    /// The caller must commit this same transaction together with any associated
    /// assistant-history change. No network I/O may happen before that commit.
    pub async fn prepare_in_transaction(
        &self,
        tx: &DatabaseTransaction,
        input: NewResponse,
    ) -> DeliveryResult<ResponseRecord> {
        validate_response(&input)?;
        validate_epoch(tx, input.key.user_id, input.source_privacy_epoch).await?;
        tx.execute(stmt(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 891129))",
            vec![
                format!(
                    "{}:{}:{}",
                    input.key.user_id, input.key.request_id, input.key.output_key
                )
                .into(),
            ],
        ))
        .await?;
        let mut intent = serde_json::to_value(&input)?;
        intent.as_object_mut().unwrap().remove("source_received_at");
        let intent_digest = digest(&intent)?;
        if let Some(old) = response_row(
            tx,
            "WHERE r.user_id=$1 AND r.request_id=$2 AND r.output_key=$3 FOR NO KEY UPDATE OF r",
            vec![
                input.key.user_id.into(),
                input.key.request_id.into(),
                input.key.output_key.clone().into(),
            ],
        )
        .await?
        {
            if old.intent_digest != intent_digest {
                return Err(DeliveryError::IdempotencyConflict);
            }
            return Ok(old);
        }
        if let Some(session) = input.conversation_id {
            let belongs = tx
                .query_one(stmt(
                    "SELECT id FROM chat_sessions WHERE id=$1 AND creator_id=$2 FOR SHARE",
                    vec![session.into(), input.key.user_id.to_string().into()],
                ))
                .await?
                .is_some();
            if !belongs {
                return Err(DeliveryError::Unauthorized(
                    "conversation belongs to another owner".into(),
                ));
            }
        }
        if let Some(source) = input.source_event_id {
            let row = tx.query_one(stmt("SELECT source_received_at,source_privacy_epoch FROM chat_messages WHERE id=$1 AND user_id=$2 AND role='user' AND ($3::uuid IS NULL OR session_id=$3) FOR SHARE",vec![source.into(),input.key.user_id.to_string().into(),input.conversation_id.into()])).await?.ok_or_else(||DeliveryError::Unauthorized("source message unavailable for owner".into()))?;
            let source_time: Option<chrono::DateTime<chrono::Utc>> =
                row.try_get("", "source_received_at")?;
            let epoch: Option<i64> = row.try_get("", "source_privacy_epoch")?;
            if source_time.map(|time| time.timestamp_micros())
                != Some(input.source_received_at.timestamp_micros())
                || epoch != input.source_privacy_epoch
                || epoch.is_none()
            {
                return Err(DeliveryError::Unauthorized(
                    "source provenance differs from admitted message".into(),
                ));
            }
        }
        let id = Uuid::new_v4();
        tx.execute(stmt("INSERT INTO delivery_responses(response_id,user_id,request_id,output_key,conversation_id,source_event_id,source_received_at,source_privacy_epoch,channel_id,kind,content,intent_digest) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",vec![id.into(),input.key.user_id.into(),input.key.request_id.into(),input.key.output_key.into(),input.conversation_id.into(),input.source_event_id.into(),input.source_received_at.into(),input.source_privacy_epoch.into(),input.channel_id.into(),input.kind.into(),input.content.into(),intent_digest.into()])).await?;
        audit(tx, id, None, "prepared", json!({})).await?;
        required_response(tx, id).await
    }

    pub async fn mark_generated_in_transaction(
        &self,
        tx: &DatabaseTransaction,
        owner: Uuid,
        response_id: Uuid,
        assistant_message_id: Uuid,
        final_content: Value,
    ) -> DeliveryResult<ResponseRecord> {
        let old = self.response_in_transaction(tx, owner, response_id).await?;
        if serde_json::to_vec(&final_content)?.len() > MAX_CONTENT_BYTES {
            return Err(DeliveryError::InvalidInput(
                "final response content exceeds bound".into(),
            ));
        }
        if old.status == ResponseStatus::Committed {
            if old.final_content.as_ref() != Some(&final_content) {
                return Err(DeliveryError::IdempotencyConflict);
            }
            return Ok(old);
        }
        let old = validate_response_source(tx, owner, response_id).await?;
        if old.status == ResponseStatus::Committed {
            if old.final_content.as_ref() != Some(&final_content) {
                return Err(DeliveryError::IdempotencyConflict);
            }
            return Ok(old);
        }
        if old.status != ResponseStatus::Pending
            || old.cancel_requested
            || assistant_message_id.is_nil()
        {
            return Err(DeliveryError::InvalidTransition(
                "response cannot commit assistant history".into(),
            ));
        }
        let session = old
            .conversation_id
            .ok_or_else(|| DeliveryError::InvalidInput("response has no conversation".into()))?;
        if tx
            .query_one(stmt(
                "SELECT id FROM chat_sessions WHERE id=$1 AND creator_id=$2 FOR SHARE",
                vec![session.into(), owner.to_string().into()],
            ))
            .await?
            .is_none()
        {
            return Err(DeliveryError::Unauthorized(
                "conversation belongs to another owner".into(),
            ));
        }
        tx.execute(stmt("UPDATE delivery_responses SET status='committed',assistant_message_id=$2,final_content=$3 WHERE response_id=$1",vec![response_id.into(),assistant_message_id.into(),final_content.into()])).await?;
        audit(
            tx,
            response_id,
            None,
            "history_disposition",
            json!({"status":"committed","assistant_message_id":assistant_message_id}),
        )
        .await?;
        required_response(tx, response_id).await
    }

    pub async fn set_response_status_in_transaction(
        &self,
        tx: &DatabaseTransaction,
        owner: Uuid,
        id: Uuid,
        status: ResponseStatus,
    ) -> DeliveryResult<ResponseRecord> {
        let old = owned_response(tx, owner, id).await?;
        if old.status == status {
            return Ok(old);
        }
        if old.status != ResponseStatus::Pending
            || matches!(status, ResponseStatus::Pending | ResponseStatus::Committed)
        {
            return Err(DeliveryError::InvalidTransition(
                "response history disposition is final".into(),
            ));
        }
        tx.execute(stmt(
            "UPDATE delivery_responses SET status=$2 WHERE response_id=$1",
            vec![id.into(), enum_name(status)?.into()],
        ))
        .await?;
        audit(
            tx,
            id,
            None,
            "history_disposition",
            json!({"status":status}),
        )
        .await?;
        required_response(tx, id).await
    }

    pub async fn prepare_delivery(
        &self,
        owner: Uuid,
        input: NewDelivery,
    ) -> DeliveryResult<DeliveryRecord> {
        if input.owner_id.is_nil()
            || !valid_key(&input.output_key)
            || serde_json::to_vec(&input.content)?.len() > MAX_CONTENT_BYTES
        {
            return Err(DeliveryError::InvalidInput(
                "invalid delivery identity or content".into(),
            ));
        }
        if input
            .identity
            .as_ref()
            .is_some_and(|identity| !identity.is_valid())
        {
            return Err(DeliveryError::InvalidInput(
                "invalid playback identity".into(),
            ));
        }
        let tx = self.db.begin().await?;
        let response = validate_response_source(&tx, owner, input.response_id).await?;
        let intent_digest = digest(
            &json!({"response_id":input.response_id,"output_key":input.output_key,"content":input.content,"identity":input.identity}),
        )?;
        if let Some(old) = delivery_row(
            &tx,
            "WHERE d.response_id=$1 AND d.output_key=$2 FOR UPDATE OF d",
            vec![input.response_id.into(), input.output_key.clone().into()],
        )
        .await?
        {
            if old.intent_digest != intent_digest {
                return Err(DeliveryError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(old);
        }
        if response.cancel_requested
            || matches!(
                response.status,
                ResponseStatus::Interrupted | ResponseStatus::Discarded
            )
        {
            return Err(DeliveryError::InvalidTransition(
                "response is retired".into(),
            ));
        }
        let response_count: i64 = tx
            .query_one(stmt(
                "SELECT COUNT(*) AS count FROM deliveries WHERE response_id=$1",
                vec![input.response_id.into()],
            ))
            .await?
            .unwrap()
            .try_get("", "count")?;
        if response_count >= 256 {
            return Err(DeliveryError::CapacityExceeded);
        }
        tx.execute(stmt(
            "SELECT pg_advisory_xact_lock($1)",
            vec![CAPACITY_LOCK.into()],
        ))
        .await?;
        let count:i64=tx.query_one(stmt("SELECT COUNT(*) AS count FROM deliveries WHERE status IN ('pending','sending','observing')",vec![])).await?.unwrap().try_get("","count")?;
        if count >= MAX_ACTIVE_DELIVERIES {
            return Err(DeliveryError::CapacityExceeded);
        }
        let id = Uuid::new_v4();
        tx.execute(stmt("INSERT INTO deliveries(delivery_id,response_id,output_key,content,playback_id,identity,intent_digest,owner_id,lease_expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,clock_timestamp()+make_interval(secs=>$9))",vec![id.into(),input.response_id.into(),input.output_key.into(),input.content.into(),input.identity.as_ref().map(|identity|identity.playback_id).into(),input.identity.map(serde_json::to_value).transpose()?.into(),intent_digest.into(),input.owner_id.into(),(LEASE_SECONDS as f64).into()])).await?;
        audit(&tx, input.response_id, Some(id), "pending", json!({})).await?;
        let result = required_delivery(&tx, id).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Only this successful transaction produces a permit. A read/retry of an
    /// already Sending row deliberately cannot recover the permit.
    pub async fn dispatch(
        &self,
        owner: Uuid,
        id: Uuid,
        instance: Uuid,
    ) -> DeliveryResult<Option<DispatchPermit>> {
        let tx = self.db.begin().await?;
        let response_id = required_delivery(&tx, id).await?.response_id;
        let response = validate_response_source(&tx, owner, response_id).await?;
        let old = locked_delivery(&tx, id).await?;
        if old.owner_id != instance {
            return Err(DeliveryError::Unauthorized(
                "delivery belongs to another runtime".into(),
            ));
        }
        if old.status != DeliveryStatus::Pending {
            tx.commit().await?;
            return Ok(None);
        }
        let dispatch_id = Uuid::new_v4();
        let changed=tx.execute(stmt("UPDATE deliveries SET status='sending',dispatch_id=$2,observation_deadline_at=clock_timestamp()+make_interval(secs=>$3),updated_at=clock_timestamp() WHERE delivery_id=$1 AND lease_expires_at>clock_timestamp() AND NOT cancel_requested",vec![id.into(),dispatch_id.into(),(OBSERVATION_SECONDS as f64).into()])).await?.rows_affected();
        if response.cancel_requested
            || matches!(
                response.status,
                ResponseStatus::Interrupted | ResponseStatus::Discarded
            )
            || changed == 0
        {
            return Ok(None);
        }
        audit(
            &tx,
            response_id,
            Some(id),
            "dispatch",
            json!({"dispatch_id":dispatch_id}),
        )
        .await?;
        tx.commit().await?;
        Ok(Some(DispatchPermit {
            delivery_id: id,
            owner_id: instance,
            dispatch_id,
        }))
    }

    /// Persists format and offer bounds before the adapter can send Start/Audio.
    pub async fn start(
        &self,
        permit: &DispatchPermit,
        sample_rate: u32,
        channels: u16,
    ) -> DeliveryResult<()> {
        if sample_rate == 0 || sample_rate > 384_000 || channels == 0 || channels > 32 {
            return Err(DeliveryError::InvalidInput(
                "invalid source PCM format".into(),
            ));
        }
        let tx = self.db.begin().await?;
        validate_delivery_source(&tx, permit.delivery_id).await?;
        let old = permitted(&tx, permit).await?;
        if old.cancel_requested {
            return Err(DeliveryError::InvalidTransition(
                "delivery cancellation requested".into(),
            ));
        }
        if let Some(rate) = old.source_sample_rate {
            if rate != sample_rate || old.source_channels != Some(channels) {
                return Err(DeliveryError::InvalidEvidence(
                    "source format changed".into(),
                ));
            }
        } else {
            tx.execute(stmt("UPDATE deliveries SET source_sample_rate=$2,source_channels=$3,updated_at=clock_timestamp() WHERE delivery_id=$1",vec![old.delivery_id.into(),(sample_rate as i32).into(),(channels as i32).into()])).await?;
            audit(
                &tx,
                old.response_id,
                Some(old.delivery_id),
                "source_format",
                json!({"sample_rate":sample_rate,"channels":channels}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Cumulative source PCM frame bound. Repeating the same bound is harmless;
    /// a lower bound or adding audio after EndAccepted is an evidence violation.
    pub async fn offered(&self, permit: &DispatchPermit, frames: u64) -> DeliveryResult<()> {
        let count = sql_count(frames)?;
        let tx = self.db.begin().await?;
        validate_delivery_source(&tx, permit.delivery_id).await?;
        let old = permitted(&tx, permit).await?;
        if old.cancel_requested {
            return Err(DeliveryError::InvalidTransition(
                "delivery cancellation requested".into(),
            ));
        }
        if old.source_sample_rate.is_none()
            || frames < old.offered_frames
            || (old.end_accepted && frames != old.offered_frames)
        {
            return Err(DeliveryError::InvalidEvidence(
                "invalid offered frame bound".into(),
            ));
        }
        if frames != old.offered_frames {
            tx.execute(stmt("UPDATE deliveries SET offered_frames=$2,updated_at=clock_timestamp() WHERE delivery_id=$1",vec![old.delivery_id.into(),count.into()])).await?;
            audit(
                &tx,
                old.response_id,
                Some(old.delivery_id),
                "offered",
                json!({"source_frames":frames}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn heartbeat(&self, permit: &DispatchPermit) -> DeliveryResult<()> {
        let tx = self.db.begin().await?;
        // A final receipt may commit just before the adapter returns. Its
        // cleanup tail needs no renewed media lease and must not be cancelled
        // merely because the already proven observation is terminal.
        let observed = required_delivery(&tx, permit.delivery_id).await?;
        if observed.owner_id == permit.owner_id
            && observed.dispatch_id == Some(permit.dispatch_id)
            && matches!(
                observed.status,
                DeliveryStatus::Submitted
                    | DeliveryStatus::Completed
                    | DeliveryStatus::Cancelled
                    | DeliveryStatus::Failed
            )
        {
            tx.commit().await?;
            return Ok(());
        }
        validate_delivery_source(&tx, permit.delivery_id).await?;
        let old = permitted(&tx, permit).await?;
        if old.cancel_requested {
            return Err(DeliveryError::InvalidTransition(
                "delivery cancellation requested by its owner".into(),
            ));
        }
        tx.execute(stmt("UPDATE deliveries SET lease_expires_at=LEAST(clock_timestamp()+make_interval(secs=>$2),observation_deadline_at),updated_at=clock_timestamp() WHERE delivery_id=$1",vec![old.delivery_id.into(),(LEASE_SECONDS as f64).into()])).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn progress(
        &self,
        permit: &DispatchPermit,
        progress: alia_core::delivery::PlaybackProgress,
    ) -> DeliveryResult<DeliveryRecord> {
        self.apply_progress(permit, progress, false).await
    }

    /// A final checkpoint can arrive after a local timeout or lease recovery.
    /// Preserve that observation in the audit log without reopening Unknown or
    /// altering its frozen prefix. It can never grant another dispatch permit.
    pub async fn terminal_checkpoint(
        &self,
        permit: &DispatchPermit,
        progress: alia_core::delivery::PlaybackProgress,
    ) -> DeliveryResult<DeliveryRecord> {
        if progress.observation_end.is_none() {
            return Err(DeliveryError::InvalidEvidence(
                "checkpoint is not final".into(),
            ));
        }
        self.apply_progress(permit, progress, true).await
    }

    async fn apply_progress(
        &self,
        permit: &DispatchPermit,
        progress: alia_core::delivery::PlaybackProgress,
        checkpoint: bool,
    ) -> DeliveryResult<DeliveryRecord> {
        let tx = self.db.begin().await?;
        let old = locked_delivery(&tx, permit.delivery_id).await?;
        if old.owner_id != permit.owner_id
            || old.dispatch_id != Some(permit.dispatch_id)
            || old.identity != Some(progress.identity)
        {
            return Err(DeliveryError::InvalidEvidence(
                "receipt has another delivery identity".into(),
            ));
        }
        if old.progress.as_ref() == Some(&progress) {
            tx.commit().await?;
            return Ok(old);
        }
        if matches!(
            progress.observation_end,
            Some(alia_core::delivery::ObservationEnd::Rejected { .. })
        ) && progress.accepted_source_frames != 0
        {
            return Err(DeliveryError::InvalidEvidence(
                "rejection after audio acceptance".into(),
            ));
        }
        if checkpoint && old.status == DeliveryStatus::Unknown {
            progress
                .validate(old.progress.as_ref(), old.offered_frames)
                .map_err(|error| DeliveryError::InvalidEvidence(error.to_string()))?;
            let payload = serde_json::to_value(&progress)?;
            if tx.query_one(stmt("SELECT event_id FROM delivery_events WHERE delivery_id=$1 AND kind='late_checkpoint' AND payload=$2 LIMIT 1", vec![old.delivery_id.into(), payload.clone().into()])).await?.is_none() {
                audit(&tx, old.response_id, Some(old.delivery_id), "late_checkpoint", payload).await?;
            }
            tx.commit().await?;
            return Ok(old);
        }
        // Repeated identical receipt is an ACK retry, but no other late receipt
        // may modify terminal facts, even if it appears to improve the evidence.
        if old.status.is_terminal() {
            return Err(DeliveryError::InvalidTransition(
                "receipt cannot change a terminal delivery".into(),
            ));
        }
        progress
            .validate(old.progress.as_ref(), old.offered_frames)
            .map_err(|error| DeliveryError::InvalidEvidence(error.to_string()))?;
        let status = match &progress.observation_end {
            Some(alia_core::delivery::ObservationEnd::SubmissionFinished) => {
                DeliveryStatus::Submitted
            }
            Some(alia_core::delivery::ObservationEnd::PresentedFinished) => {
                DeliveryStatus::Completed
            }
            Some(alia_core::delivery::ObservationEnd::Cancelled { .. }) => {
                DeliveryStatus::Cancelled
            }
            Some(
                alia_core::delivery::ObservationEnd::Failed { .. }
                | alia_core::delivery::ObservationEnd::Rejected { .. },
            ) => DeliveryStatus::Failed,
            Some(alia_core::delivery::ObservationEnd::Unknown { .. }) => DeliveryStatus::Unknown,
            None if progress.end_accepted => DeliveryStatus::Observing,
            None => DeliveryStatus::Sending,
        };
        tx.execute(stmt("UPDATE deliveries SET status=$2,accepted_frames=$3,submitted_frames=$4,presented_frames=$5,end_accepted=$6,receipt_sequence=$7,progress=$8,outcome=$9,updated_at=clock_timestamp() WHERE delivery_id=$1",vec![old.delivery_id.into(),enum_name(status)?.into(),sql_count(progress.accepted_source_frames)?.into(),progress.submitted_source_frames.map(sql_count).transpose()?.into(),progress.presented_source_frames.map(sql_count).transpose()?.into(),progress.end_accepted.into(),sql_count(progress.receipt_sequence)?.into(),serde_json::to_value(&progress)?.into(),progress.observation_end.as_ref().map(serde_json::to_value).transpose()?.into()])).await?;
        audit(
            &tx,
            old.response_id,
            Some(old.delivery_id),
            "progress",
            serde_json::to_value(&progress)?,
        )
        .await?;
        let record = required_delivery(&tx, old.delivery_id).await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Local timeout/transport error has no new device evidence. Preserve every
    /// known prefix and record Unknown; never fabricate a terminal receipt.
    pub async fn finish_unknown(
        &self,
        permit: &DispatchPermit,
        reason: &str,
    ) -> DeliveryResult<DeliveryRecord> {
        let tx = self.db.begin().await?;
        let old = locked_delivery(&tx, permit.delivery_id).await?;
        if old.owner_id != permit.owner_id || old.dispatch_id != Some(permit.dispatch_id) {
            return Err(DeliveryError::InvalidTransition(
                "not this dispatch owner".into(),
            ));
        }
        if old.status.is_terminal() {
            tx.commit().await?;
            return Ok(old);
        }
        let outcome = json!({"kind":"unknown","reason":reason});
        tx.execute(stmt("UPDATE deliveries SET status='unknown',outcome=$2,updated_at=clock_timestamp() WHERE delivery_id=$1",vec![old.delivery_id.into(),outcome.clone().into()])).await?;
        audit(
            &tx,
            old.response_id,
            Some(old.delivery_id),
            "unknown",
            outcome,
        )
        .await?;
        let record = required_delivery(&tx, old.delivery_id).await?;
        tx.commit().await?;
        Ok(record)
    }

    pub async fn fail_pending(
        &self,
        owner: Uuid,
        id: Uuid,
        reason: &str,
    ) -> DeliveryResult<DeliveryRecord> {
        let tx = self.db.begin().await?;
        let response_id = required_delivery(&tx, id).await?.response_id;
        owned_response(&tx, owner, response_id).await?;
        let old = locked_delivery(&tx, id).await?;
        if old.status != DeliveryStatus::Pending {
            return Err(DeliveryError::InvalidTransition(
                "failure without observation requires undispatched delivery".into(),
            ));
        }
        let outcome = json!({"kind":"failed","reason":reason,"evidence":"undispatched"});
        tx.execute(stmt("UPDATE deliveries SET status='failed',outcome=$2,updated_at=clock_timestamp() WHERE delivery_id=$1",vec![id.into(),outcome.clone().into()])).await?;
        audit(&tx, response_id, Some(id), "failed", outcome).await?;
        let record = required_delivery(&tx, id).await?;
        tx.commit().await?;
        Ok(record)
    }

    pub async fn request_cancel(
        &self,
        owner: Uuid,
        response_id: Uuid,
        reason: &str,
    ) -> DeliveryResult<Vec<DeliveryRecord>> {
        let tx = self.db.begin().await?;
        let response = owned_response(&tx, owner, response_id).await?;
        tx.execute(stmt(
            "UPDATE delivery_responses SET cancel_requested=TRUE,status=CASE WHEN status='pending' THEN 'interrupted' ELSE status END WHERE response_id=$1",
            vec![response_id.into()],
        ))
        .await?;
        if !response.cancel_requested {
            audit(
                &tx,
                response_id,
                None,
                "response_cancel_requested",
                json!({"reason":reason}),
            )
            .await?;
        }
        let records = delivery_many(
            &tx,
            "WHERE d.response_id=$1 ORDER BY d.delivery_id FOR UPDATE OF d",
            vec![response_id.into()],
        )
        .await?;
        for old in &records {
            if old.status.is_terminal() {
                continue;
            }
            let pending = old.status == DeliveryStatus::Pending;
            tx.execute(stmt("UPDATE deliveries SET cancel_requested=TRUE,status=CASE WHEN status='pending' THEN 'cancelled' ELSE status END,outcome=CASE WHEN status='pending' THEN $2 ELSE outcome END,updated_at=clock_timestamp() WHERE delivery_id=$1",vec![old.delivery_id.into(),json!({"reason":reason,"evidence":"undispatched"}).into()])).await?;
            audit(
                &tx,
                response_id,
                Some(old.delivery_id),
                if pending {
                    "cancelled"
                } else {
                    "cancel_requested"
                },
                json!({"reason":reason}),
            )
            .await?;
        }
        let result = delivery_many(
            &tx,
            "WHERE d.response_id=$1 ORDER BY d.created_at",
            vec![response_id.into()],
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Only expired leases are recovered. This never returns media to send, and
    /// never modifies another live instance's nonexpired owner lease.
    pub async fn recover_expired(&self, limit: u32) -> DeliveryResult<usize> {
        let tx = self.db.begin().await?;
        let rows=delivery_many(&tx,"WHERE d.status IN ('pending','sending','observing') AND d.lease_expires_at<=clock_timestamp() ORDER BY d.lease_expires_at LIMIT $1 FOR UPDATE OF d SKIP LOCKED",vec![(limit.clamp(1,256) as i64).into()]).await?;
        for old in &rows {
            let status = if old.status == DeliveryStatus::Pending {
                "cancelled"
            } else {
                "unknown"
            };
            let outcome = json!({"reason":"owner_lease_expired","evidence":if old.status==DeliveryStatus::Pending {"undispatched"} else {"observation_lost"}});
            tx.execute(stmt("UPDATE deliveries SET status=$2,outcome=$3,updated_at=clock_timestamp() WHERE delivery_id=$1",vec![old.delivery_id.into(),status.into(),outcome.clone().into()])).await?;
            audit(&tx, old.response_id, Some(old.delivery_id), status, outcome).await?;
        }
        tx.commit().await?;
        Ok(rows.len())
    }

    pub async fn response_in_transaction(
        &self,
        tx: &DatabaseTransaction,
        owner: Uuid,
        id: Uuid,
    ) -> DeliveryResult<ResponseRecord> {
        response_row(
            tx,
            "WHERE r.response_id=$1 AND r.user_id=$2",
            vec![id.into(), owner.into()],
        )
        .await?
        .ok_or(DeliveryError::NotFound)
    }

    pub async fn response(&self, owner: Uuid, id: Uuid) -> DeliveryResult<ResponseRecord> {
        response_row(
            &self.db,
            "WHERE r.response_id=$1 AND r.user_id=$2",
            vec![id.into(), owner.into()],
        )
        .await?
        .ok_or(DeliveryError::NotFound)
    }
    pub async fn get(&self, owner: Uuid, id: Uuid) -> DeliveryResult<DeliveryRecord> {
        delivery_row(&self.db,"JOIN delivery_responses r ON r.response_id=d.response_id WHERE d.delivery_id=$1 AND r.user_id=$2",vec![id.into(),owner.into()]).await?.ok_or(DeliveryError::NotFound)
    }
    pub async fn find(
        &self,
        owner: Uuid,
        response_id: Uuid,
        output_key: &str,
    ) -> DeliveryResult<Option<DeliveryRecord>> {
        delivery_row(&self.db,"JOIN delivery_responses r ON r.response_id=d.response_id WHERE d.response_id=$1 AND d.output_key=$2 AND r.user_id=$3",vec![response_id.into(),output_key.into(),owner.into()]).await
    }
    pub async fn list(
        &self,
        owner: Uuid,
        response_id: Uuid,
    ) -> DeliveryResult<Vec<DeliveryRecord>> {
        self.response(owner, response_id).await?;
        delivery_many(
            &self.db,
            "WHERE d.response_id=$1 ORDER BY d.created_at,d.delivery_id LIMIT 256",
            vec![response_id.into()],
        )
        .await
    }
    pub async fn events(
        &self,
        owner: Uuid,
        response_id: Uuid,
        after: i64,
        limit: u32,
    ) -> DeliveryResult<Vec<DeliveryEvent>> {
        self.response(owner, response_id).await?;
        rows(&self.db,"SELECT to_jsonb(e) AS record FROM delivery_events e WHERE response_id=$1 AND event_id>$2 ORDER BY event_id LIMIT $3",vec![response_id.into(),after.into(),(limit.clamp(1,256) as i64).into()]).await
    }
}

async fn authorize(db: &impl ConnectionTrait, owner: Uuid) -> DeliveryResult<()> {
    if owner.is_nil()
        || db
            .query_one(stmt(
                "SELECT id FROM users WHERE id=$1 AND status='active' FOR SHARE",
                vec![owner.into()],
            ))
            .await?
            .is_none()
    {
        return Err(DeliveryError::Unauthorized(
            "owner is not an active account".into(),
        ));
    }
    Ok(())
}
/// Same lock order as source admission/erasure: account, privacy state, then
/// response and delivery. This checks the original stamp without refreshing it.
async fn validate_epoch(
    db: &impl ConnectionTrait,
    owner: Uuid,
    expected: Option<i64>,
) -> DeliveryResult<()> {
    authorize(db, owner).await?;
    let expected = expected.ok_or_else(|| {
        DeliveryError::Unauthorized("response has no admitted source privacy epoch".into())
    })?;
    let row = db
        .query_one(stmt(
            "SELECT privacy_epoch FROM memory_privacy_state WHERE user_id=$1 FOR UPDATE",
            vec![owner.into()],
        ))
        .await?
        .ok_or_else(|| DeliveryError::Unauthorized("source admission state missing".into()))?;
    let current: i64 = row.try_get("", "privacy_epoch")?;
    if current != expected {
        return Err(DeliveryError::Unauthorized(
            "response source privacy epoch changed".into(),
        ));
    }
    Ok(())
}
async fn validate_response_source(
    db: &impl ConnectionTrait,
    owner: Uuid,
    id: Uuid,
) -> DeliveryResult<ResponseRecord> {
    let old = response_row(
        db,
        "WHERE r.response_id=$1 AND r.user_id=$2",
        vec![id.into(), owner.into()],
    )
    .await?
    .ok_or(DeliveryError::NotFound)?;
    validate_epoch(db, owner, old.source_privacy_epoch).await?;
    owned_response(db, owner, id).await
}
async fn validate_delivery_source(
    db: &impl ConnectionTrait,
    id: Uuid,
) -> DeliveryResult<ResponseRecord> {
    let delivery = required_delivery(db, id).await?;
    let response = required_response(db, delivery.response_id).await?;
    validate_response_source(db, response.key.user_id, response.response_id).await
}

async fn owned_response(
    db: &impl ConnectionTrait,
    owner: Uuid,
    id: Uuid,
) -> DeliveryResult<ResponseRecord> {
    response_row(
        db,
        "WHERE r.response_id=$1 AND r.user_id=$2 FOR NO KEY UPDATE OF r",
        vec![id.into(), owner.into()],
    )
    .await?
    .ok_or(DeliveryError::NotFound)
}
async fn required_response(db: &impl ConnectionTrait, id: Uuid) -> DeliveryResult<ResponseRecord> {
    response_row(db, "WHERE r.response_id=$1", vec![id.into()])
        .await?
        .ok_or(DeliveryError::NotFound)
}
async fn required_delivery(db: &impl ConnectionTrait, id: Uuid) -> DeliveryResult<DeliveryRecord> {
    delivery_row(db, "WHERE d.delivery_id=$1", vec![id.into()])
        .await?
        .ok_or(DeliveryError::NotFound)
}
async fn locked_delivery(db: &impl ConnectionTrait, id: Uuid) -> DeliveryResult<DeliveryRecord> {
    delivery_row(
        db,
        "WHERE d.delivery_id=$1 FOR UPDATE OF d",
        vec![id.into()],
    )
    .await?
    .ok_or(DeliveryError::NotFound)
}
async fn permitted(
    db: &impl ConnectionTrait,
    permit: &DispatchPermit,
) -> DeliveryResult<DeliveryRecord> {
    let old = locked_delivery(db, permit.delivery_id).await?;
    if old.owner_id != permit.owner_id
        || old.dispatch_id != Some(permit.dispatch_id)
        || old.status.is_terminal()
    {
        return Err(DeliveryError::InvalidTransition(
            "dispatch permit is no longer active".into(),
        ));
    }
    let valid=db.query_one(stmt("SELECT delivery_id FROM deliveries WHERE delivery_id=$1 AND lease_expires_at>clock_timestamp() AND observation_deadline_at>clock_timestamp()",vec![old.delivery_id.into()])).await?.is_some();
    if !valid {
        return Err(DeliveryError::InvalidTransition(
            "delivery owner lease or observation deadline expired".into(),
        ));
    }
    Ok(old)
}
async fn response_row(
    db: &impl ConnectionTrait,
    suffix: &str,
    values: Vec<SqlValue>,
) -> DeliveryResult<Option<ResponseRecord>> {
    row(db, &format!("{RESPONSE}{suffix}"), values).await
}
async fn delivery_row(
    db: &impl ConnectionTrait,
    suffix: &str,
    values: Vec<SqlValue>,
) -> DeliveryResult<Option<DeliveryRecord>> {
    row(db, &format!("{DELIVERY}{suffix}"), values).await
}
async fn delivery_many(
    db: &impl ConnectionTrait,
    suffix: &str,
    values: Vec<SqlValue>,
) -> DeliveryResult<Vec<DeliveryRecord>> {
    rows(db, &format!("{DELIVERY}{suffix}"), values).await
}
async fn row<T: DeserializeOwned>(
    db: &impl ConnectionTrait,
    sql: &str,
    values: Vec<SqlValue>,
) -> DeliveryResult<Option<T>> {
    db.query_one(stmt(sql, values))
        .await?
        .map(|row| Ok(serde_json::from_value(row.try_get::<Value>("", "record")?)?))
        .transpose()
}
async fn rows<T: DeserializeOwned>(
    db: &impl ConnectionTrait,
    sql: &str,
    values: Vec<SqlValue>,
) -> DeliveryResult<Vec<T>> {
    db.query_all(stmt(sql, values))
        .await?
        .into_iter()
        .map(|row| Ok(serde_json::from_value(row.try_get::<Value>("", "record")?)?))
        .collect()
}
async fn audit(
    db: &impl ConnectionTrait,
    response: Uuid,
    delivery: Option<Uuid>,
    kind: &str,
    payload: Value,
) -> DeliveryResult<()> {
    db.execute(stmt(
        "INSERT INTO delivery_events(response_id,delivery_id,kind,payload) VALUES($1,$2,$3,$4)",
        vec![
            response.into(),
            delivery.into(),
            kind.into(),
            payload.into(),
        ],
    ))
    .await?;
    Ok(())
}
fn stmt(sql: &str, values: Vec<SqlValue>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}
fn digest(value: &impl Serialize) -> DeliveryResult<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn enum_name(value: impl Serialize) -> DeliveryResult<String> {
    Ok(serde_json::to_value(value)?.as_str().unwrap().to_string())
}
fn sql_count(value: u64) -> DeliveryResult<i64> {
    i64::try_from(value)
        .map_err(|_| DeliveryError::InvalidEvidence("frame counter overflow".into()))
}
fn valid_key(key: &str) -> bool {
    !key.trim().is_empty() && key.len() <= 256
}
fn validate_response(input: &NewResponse) -> DeliveryResult<()> {
    if input.key.user_id.is_nil()
        || input.key.request_id.is_nil()
        || !valid_key(&input.key.output_key)
        || !valid_key(&input.channel_id)
        || !valid_key(&input.kind)
        || input.source_privacy_epoch.is_some_and(|epoch| epoch < 0)
        || serde_json::to_vec(&input.content)?.len() > MAX_CONTENT_BYTES
    {
        return Err(DeliveryError::InvalidInput(
            "invalid response identity, provenance or content".into(),
        ));
    }
    Ok(())
}
