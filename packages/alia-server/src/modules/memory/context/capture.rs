use super::{repository::*, *};
use crate::providers::adapter::{ChatMessage, MemoryWriteContext, MemoryWriteOrigin};
use sea_orm::{ConnectionTrait, IsolationLevel, TransactionTrait};
use serde_json::json;
use uuid::Uuid;

impl ContextService {
    /// Freeze the actual canonical rows for a background derivation. The caller
    /// must build its model prompt from these returned rows, never an earlier
    /// unvalidated copy. IDs/revisions select inputs; they cannot supply content.
    pub async fn materialize_memory_capture(
        &self,
        snapshot: &MemoryWriteContext,
        origin: MemoryWriteOrigin,
    ) -> ContextResult<CapturedMemoryContext> {
        let (usage, purpose, purpose_name) = match origin {
            MemoryWriteOrigin::Automatic => (
                ContextUse::AutomaticMemoryWrite,
                ContextPurpose::MemoryCapture,
                "memory_capture",
            ),
            MemoryWriteOrigin::Maintenance => (
                ContextUse::MemoryMaintenance,
                ContextPurpose::MemoryMaintenance,
                "memory_maintenance",
            ),
            _ => {
                return Err(ContextError::Revoked(
                    "background capture requires an explicit automatic or maintenance origin"
                        .into(),
                )
                .into());
            }
        };
        let expected = snapshot.policy_version.ok_or_else(|| {
            ContextError::Revoked("capture has no original persistent policy".into())
        })?;
        if snapshot.source_event_ids.len() + snapshot.source_memories.len() > MAX_EVIDENCE_REFS
            || snapshot.source_event_ids.len() > MAX_HISTORY_MESSAGES
        {
            return Err(ContextError::BoundsExceeded.into());
        }
        let tx = self
            .db
            .begin_with_config(Some(IsolationLevel::RepeatableRead), None)
            .await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let source = ContextSource {
            owner_id: snapshot.user_id,
            request_id: Uuid::new_v4(),
            channel_id: "memory-capture".into(),
            conversation_id: None,
            source_event_id: None,
            source_received_at: snapshot.source_received_at,
            source_privacy_epoch: snapshot.privacy_epoch,
        };
        let policy = lock_authority(&tx, &source, expected).await?;
        if !policy.policy.allows(usage) {
            return Err(
                ContextError::Revoked("policy disallows this capture origin".into()).into(),
            );
        }
        let mut lease = ContextLease {
            id: Uuid::new_v4(),
            purpose,
            materialized_digest: String::new(),
            source,
            policy_version: expected,
            dependencies: Vec::new(),
            valid_until: None,
        };
        if let Some(original) = snapshot.context_id {
            let original = validate_lease(&tx, snapshot.user_id, original, usage)
                .await?
                .lease;
            merge(&mut lease.dependencies, &original.dependencies)?;
            lease.valid_until = original.valid_until;
        }
        let mut ids = snapshot.source_event_ids.clone();
        ids.sort_unstable();
        ids.dedup();
        let mut messages = Vec::new();
        for id in ids {
            let row=tx.query_one(stmt("SELECT m.session_id,m.content,m.source_received_at,m.created_at FROM chat_messages m JOIN chat_sessions s ON s.id=m.session_id WHERE m.id=$1 AND m.user_id=$2 AND s.creator_id=$2 AND m.role='user' AND m.source_privacy_epoch=$3 AND m.source_received_at IS NOT NULL FOR SHARE OF m",vec![id.into(),snapshot.user_id.to_string().into(),snapshot.privacy_epoch.into()])).await?.ok_or_else(||ContextError::Revoked("capture user source is unavailable".into()))?;
            let received =
                row.try_get::<chrono::DateTime<chrono::Utc>>("", "source_received_at")?;
            // Ingress and DB timestamps may come from different clocks. The
            // persisted identity and epoch supply authority, never clock order.
            if snapshot
                .exact_source_received_at
                .is_some_and(|(exact, time)| {
                    exact == id && time.timestamp_micros() != received.timestamp_micros()
                })
            {
                return Err(ContextError::Revoked(
                    "capture source differs from its exact original stamp".into(),
                )
                .into());
            }
            let content: String = row.try_get("", "content")?;
            merge(
                &mut lease.dependencies,
                &[ContextEvidence::UserMessage {
                    id,
                    session_id: row.try_get("", "session_id")?,
                    source_privacy_epoch: snapshot.privacy_epoch,
                    content_digest: digest(&json!(content))?,
                }],
            )?;
            messages.push((
                row.try_get::<chrono::DateTime<chrono::Utc>>("", "created_at")?,
                ContextHistoryMessage {
                    id,
                    message: ChatMessage::new("user", &content),
                },
            ));
        }
        messages.sort_by_key(|(created, item)| (*created, item.id));
        let messages = messages
            .into_iter()
            .map(|(_, item)| item)
            .collect::<Vec<_>>();
        let mut selected = snapshot.source_memories.clone();
        selected.sort_unstable();
        selected.dedup();
        let mut memories = Vec::new();
        for (id, revision) in selected {
            let id = Uuid::parse_str(&id)?;
            let memory = crate::providers::memory::simple::context_node(&tx, snapshot.user_id, id)
                .await?
                .ok_or_else(|| {
                    ContextError::Revoked("capture memory source is unavailable".into())
                })?;
            if memory.revision != revision {
                return Err(ContextError::Revoked("capture memory revision changed".into()).into());
            }
            merge(
                &mut lease.dependencies,
                &[ContextEvidence::Memory { id, revision }],
            )?;
            lease.valid_until = earliest(lease.valid_until, memory.expires_at);
            memories.push(memory);
        }
        if lease.dependencies.is_empty() {
            return Err(ContextError::Revoked(
                "capture requires actual canonical source rows".into(),
            )
            .into());
        }
        let payload = json!({"messages":messages.iter().map(|item|json!({"id":item.id,"content":item.message.content})).collect::<Vec<_>>(),"memories":memories});
        if serde_json::to_vec(&payload)?.len() > MAX_CONTEXT_BYTES {
            return Err(ContextError::BoundsExceeded.into());
        }
        validate_evidence(&tx, &lease).await?;
        persist(&tx, &mut lease, "capture", purpose_name, &payload).await?;
        let id = lease.id;
        tx.commit()
            .await
            .map_err(|_| ContextError::CommitUnknown { context_id: id })?;
        Ok(CapturedMemoryContext {
            lease,
            messages,
            memories,
        })
    }
}
