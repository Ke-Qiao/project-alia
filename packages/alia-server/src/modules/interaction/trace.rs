use super::services::InteractionServices;
use crate::providers::adapter::ConversationEventEntry;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub(super) struct AgentTrace {
    pub(super) session_id: Uuid,
    pub(super) turn_id: Uuid,
    pub(super) next_sequence: i32,
    pub(super) tts_sequence: Arc<std::sync::atomic::AtomicI32>,
}

impl AgentTrace {
    pub(super) fn new(session_id: Uuid) -> Self {
        Self {
            session_id,
            turn_id: Uuid::new_v4(),
            next_sequence: 0,
            tts_sequence: Arc::new(std::sync::atomic::AtomicI32::new(10_000)),
        }
    }

    pub(super) async fn record(
        &mut self,
        modules: &InteractionServices,
        event_type: impl Into<String>,
        actor: impl Into<String>,
        summary: Option<String>,
        payload: serde_json::Value,
    ) {
        let event = ConversationEventEntry {
            turn_id: self.turn_id,
            sequence: self.next_sequence,
            event_type: event_type.into(),
            actor: actor.into(),
            summary,
            payload,
        };
        self.next_sequence += 1;
        if let Err(error) = modules
            .conversation
            .append_event(self.session_id, event)
            .await
        {
            tracing::debug!(error = ?error, "failed to append conversation event");
        }
    }
}
