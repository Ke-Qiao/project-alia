use super::*;
use alia_core::delivery::ObservationEnd;
use alia_core::text_delivery::{
    MAX_TEXT_BYTES, MAX_TEXT_CHUNK_BYTES, MAX_TEXT_CHUNKS, TextChunk, TextProgress,
};

impl DeliveryRepository {
    pub async fn text_start(&self, permit: &DispatchPermit) -> DeliveryResult<()> {
        let tx = self.db.begin().await?;
        let response = validate_delivery_source(&tx, permit.delivery_id).await?;
        reject_invitation_text(&response)?;
        let old = permitted(&tx, permit).await?;
        text_state(&old)?;
        if old.cancel_requested {
            return Err(DeliveryError::InvalidTransition("text cancelled".into()));
        }
        audit(
            &tx,
            old.response_id,
            Some(old.delivery_id),
            "text_start",
            json!({}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn offer_text_chunk(
        &self,
        permit: &DispatchPermit,
        chunk: &TextChunk,
    ) -> DeliveryResult<()> {
        if chunk.text.is_empty()
            || chunk.text.len() > MAX_TEXT_CHUNK_BYTES
            || chunk.sequence >= MAX_TEXT_CHUNKS
            || chunk
                .utf8_offset
                .checked_add(chunk.text.len() as u64)
                .is_none_or(|n| n > MAX_TEXT_BYTES)
        {
            return Err(DeliveryError::InvalidInput(
                "text chunk exceeds bounded UTF-8 stream contract".into(),
            ));
        }
        let tx = self.db.begin().await?;
        let response = validate_delivery_source(&tx, permit.delivery_id).await?;
        reject_invitation_text(&response)?;
        if response.kind == "source_notice" {
            let text = response
                .content
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    DeliveryError::InvalidEvidence("notice has no fixed literal".into())
                })?;
            let start = usize::try_from(chunk.utf8_offset).unwrap_or(usize::MAX);
            let end = start
                .checked_add(chunk.text.len())
                .ok_or_else(|| DeliveryError::InvalidEvidence("notice offset overflow".into()))?;
            if text.get(start..end) != Some(chunk.text.as_str()) {
                return Err(DeliveryError::Unauthorized(
                    "notice chunk differs from its fixed literal".into(),
                ));
            }
        }
        let old = permitted(&tx, permit).await?;
        let state = text_state(&old)?;
        if old.cancel_requested || state.end_offered {
            return Err(DeliveryError::InvalidTransition(
                "text stream is closed".into(),
            ));
        }
        if chunk.sequence < state.offered_chunks {
            let previous:TextChunk=row(&tx,"SELECT to_jsonb(c) AS record FROM delivery_text_chunks c WHERE delivery_id=$1 AND sequence=$2",vec![old.delivery_id.into(),sql_count(chunk.sequence)?.into()]).await?.ok_or_else(||DeliveryError::InvalidEvidence("missing immutable text chunk".into()))?;
            if previous != *chunk {
                return Err(DeliveryError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(());
        }
        if chunk.sequence != state.offered_chunks || chunk.utf8_offset != state.offered_utf8_bytes {
            return Err(DeliveryError::InvalidEvidence(
                "text sequence or source offset is not contiguous".into(),
            ));
        }
        tx.execute(stmt("INSERT INTO delivery_text_chunks(delivery_id,sequence,utf8_offset,text) VALUES($1,$2,$3,$4)",vec![old.delivery_id.into(),sql_count(chunk.sequence)?.into(),sql_count(chunk.utf8_offset)?.into(),chunk.text.clone().into()])).await?;
        tx.execute(stmt("UPDATE deliveries SET text_offered_utf8_bytes=$2,text_offered_chunks=text_offered_chunks+1,updated_at=clock_timestamp() WHERE delivery_id=$1",vec![old.delivery_id.into(),sql_count(chunk.utf8_offset+chunk.text.len() as u64)?.into()])).await?;
        audit(
            &tx,
            old.response_id,
            Some(old.delivery_id),
            "text_offered",
            serde_json::to_value(chunk)?,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn offer_text_end(
        &self,
        permit: &DispatchPermit,
        chunks: u64,
        bytes: u64,
    ) -> DeliveryResult<()> {
        let tx = self.db.begin().await?;
        let response = validate_delivery_source(&tx, permit.delivery_id).await?;
        reject_invitation_text(&response)?;
        if response.kind == "source_notice"
            && response
                .content
                .get("text")
                .and_then(Value::as_str)
                .map(|s| s.len() as u64)
                != Some(bytes)
        {
            return Err(DeliveryError::Unauthorized(
                "notice End differs from its complete fixed literal".into(),
            ));
        }
        let old = permitted(&tx, permit).await?;
        let state = text_state(&old)?;
        if old.cancel_requested
            || chunks != state.offered_chunks
            || bytes != state.offered_utf8_bytes
        {
            return Err(DeliveryError::InvalidEvidence(
                "text End does not match admitted chunks".into(),
            ));
        }
        if !state.end_offered {
            tx.execute(stmt("UPDATE deliveries SET text_end_offered=TRUE,updated_at=clock_timestamp() WHERE delivery_id=$1",vec![old.delivery_id.into()])).await?;
            audit(
                &tx,
                old.response_id,
                Some(old.delivery_id),
                "text_end_offered",
                json!({"chunks":chunks,"utf8_bytes":bytes}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn text_progress(
        &self,
        permit: &DispatchPermit,
        progress: TextProgress,
        checkpoint: bool,
    ) -> DeliveryResult<DeliveryRecord> {
        let tx = self.db.begin().await?;
        let old = locked_delivery(&tx, permit.delivery_id).await?;
        let state = text_state(&old)?;
        if old.owner_id != permit.owner_id
            || old.dispatch_id != Some(permit.dispatch_id)
            || state.identity != Some(progress.identity)
        {
            return Err(DeliveryError::InvalidEvidence(
                "text receipt has another dispatch identity".into(),
            ));
        }
        if state.progress.as_ref() == Some(&progress) {
            tx.commit().await?;
            return Ok(old);
        }
        if checkpoint && progress.observation_end.is_none() {
            return Err(DeliveryError::InvalidEvidence(
                "text checkpoint is not final".into(),
            ));
        }
        progress
            .validate(
                state.progress.as_ref(),
                state.offered_utf8_bytes,
                state.end_offered,
            )
            .map_err(|e| DeliveryError::InvalidEvidence(e.into()))?;
        let payload = serde_json::to_value(&progress)?;
        if checkpoint && old.status == DeliveryStatus::Unknown {
            if tx.query_one(stmt("SELECT event_id FROM delivery_events WHERE delivery_id=$1 AND kind='late_text_checkpoint' AND payload=$2 LIMIT 1",vec![old.delivery_id.into(),payload.clone().into()])).await?.is_none() {
                audit(&tx,old.response_id,Some(old.delivery_id),"late_text_checkpoint",payload).await?;
            }
            tx.commit().await?;
            return Ok(old);
        }
        if old.status.is_terminal() {
            return Err(DeliveryError::InvalidTransition(
                "text receipt cannot reopen terminal delivery".into(),
            ));
        }
        let status = match &progress.observation_end {
            Some(ObservationEnd::SubmissionFinished) => DeliveryStatus::Submitted,
            Some(ObservationEnd::PresentedFinished) => {
                unreachable!("text validation forbids presentation claims")
            }
            Some(ObservationEnd::Cancelled { .. }) => DeliveryStatus::Cancelled,
            Some(ObservationEnd::Failed { .. } | ObservationEnd::Rejected { .. }) => {
                DeliveryStatus::Failed
            }
            Some(ObservationEnd::Unknown { .. }) => DeliveryStatus::Unknown,
            None if progress.end_accepted => DeliveryStatus::Observing,
            None => DeliveryStatus::Sending,
        };
        tx.execute(stmt("UPDATE deliveries SET status=$2,text_progress=$3,receipt_sequence=$4,outcome=$5,updated_at=clock_timestamp() WHERE delivery_id=$1",vec![old.delivery_id.into(),enum_name(status)?.into(),payload.clone().into(),sql_count(progress.receipt_sequence)?.into(),progress.observation_end.as_ref().map(serde_json::to_value).transpose()?.into()])).await?;
        audit(
            &tx,
            old.response_id,
            Some(old.delivery_id),
            "text_progress",
            payload,
        )
        .await?;
        let record = required_delivery(&tx, old.delivery_id).await?;
        tx.commit().await?;
        Ok(record)
    }

    pub async fn text_chunks(
        &self,
        owner: Uuid,
        id: Uuid,
        after: Option<u64>,
        limit: u32,
    ) -> DeliveryResult<Vec<TextChunk>> {
        let record = self.get(owner, id).await?;
        text_state(&record)?;
        rows(&self.db,"SELECT to_jsonb(c) AS record FROM delivery_text_chunks c WHERE delivery_id=$1 AND ($2::bigint IS NULL OR sequence>$2) ORDER BY sequence LIMIT $3",vec![id.into(),after.map(sql_count).transpose()?.into(),(limit.clamp(1,256) as i64).into()]).await
    }
}

fn text_state(record: &DeliveryRecord) -> DeliveryResult<&TextDeliveryState> {
    record
        .text
        .as_ref()
        .filter(|_| record.media_kind == DeliveryMedium::Text)
        .ok_or_else(|| DeliveryError::InvalidTransition("delivery is not text".into()))
}
