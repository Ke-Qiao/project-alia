//! Conversation history and admitted input provenance.
mod service;

pub use service::ConversationService;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::providers::adapter::ChatMessage;

#[async_trait]
pub trait ConversationStore: Send + Sync {
    /// Get the active session ID for a channel/user combo, creating one if none exists.
    async fn get_or_create_active_session(
        &self,
        channel_id: &str,
        user_id: &str,
    ) -> anyhow::Result<Uuid>;

    /// Create a new session explicitly and return its ID.
    async fn create_session(&self, channel_id: &str, user_id: &str) -> anyhow::Result<Uuid>;

    /// List all sessions for a channel/user, ordered by recency.
    async fn list_sessions(
        &self,
        channel_id: &str,
        user_id: &str,
    ) -> anyhow::Result<Vec<(Uuid, Option<String>, chrono::DateTime<chrono::Utc>)>>;

    /// Set the active session for a channel/user.
    async fn set_active_session(
        &self,
        channel_id: &str,
        user_id: &str,
        session_id: Uuid,
    ) -> anyhow::Result<()>;

    /// Fetch the last N messages from a session.
    async fn get_history(&self, session_id: Uuid, limit: u64) -> anyhow::Result<Vec<ChatMessage>>;

    /// Append a message to a session.
    async fn append_message(&self, session_id: Uuid, message: ChatMessage) -> anyhow::Result<()>;

    /// Persist trusted user input and return the actual source record identity.
    async fn append_sourced_message(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        message: ChatMessage,
        received_at: chrono::DateTime<chrono::Utc>,
        source_privacy_epoch: i64,
        source_relationship_epoch: i64,
    ) -> anyhow::Result<Uuid>;

    /// Persist a traceable agent event, reporting any failure to the caller.
    async fn append_event(
        &self,
        session_id: Uuid,
        event: ConversationEventEntry,
    ) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEventEntry {
    pub turn_id: Uuid,
    pub sequence: i32,
    pub event_type: String,
    pub actor: String,
    pub summary: Option<String>,
    pub payload: serde_json::Value,
}
