use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const MAX_CONTENT_BYTES: usize = 256 * 1024;
pub const MAX_ACTIVE_DELIVERIES: i64 = 64;
pub const LEASE_SECONDS: i64 = 15;
pub const OBSERVATION_SECONDS: i64 = 120;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResponseKey {
    pub user_id: Uuid,
    pub request_id: Uuid,
    pub output_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NewResponse {
    pub key: ResponseKey,
    pub conversation_id: Option<Uuid>,
    pub source_event_id: Option<Uuid>,
    pub source_received_at: DateTime<Utc>,
    pub source_privacy_epoch: Option<i64>,
    pub channel_id: String,
    pub kind: String,
    pub content: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Pending,
    Committed,
    Interrupted,
    Discarded,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResponseRecord {
    pub response_id: Uuid,
    pub key: ResponseKey,
    pub conversation_id: Option<Uuid>,
    pub assistant_message_id: Option<Uuid>,
    pub source_event_id: Option<Uuid>,
    pub source_received_at: DateTime<Utc>,
    pub source_privacy_epoch: Option<i64>,
    pub channel_id: String,
    pub kind: String,
    pub content: Value,
    pub final_content: Option<Value>,
    pub intent_digest: String,
    pub status: ResponseStatus,
    pub cancel_requested: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Pending,
    Sending,
    Observing,
    Submitted,
    Completed,
    Cancelled,
    Failed,
    Unknown,
}

impl DeliveryStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending | Self::Sending | Self::Observing)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NewDelivery {
    pub response_id: Uuid,
    pub output_key: String,
    /// Exact fragment/representation delivered; immutable across retransmission.
    pub content: Value,
    #[schema(value_type = Option<serde_json::Value>)]
    pub identity: Option<alia_core::delivery::PlaybackIdentity>,
    pub owner_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeliveryRecord {
    pub delivery_id: Uuid,
    pub response_id: Uuid,
    pub output_key: String,
    pub media_kind: DeliveryMedium,
    pub content: Value,
    #[schema(value_type = Option<serde_json::Value>)]
    pub identity: Option<alia_core::delivery::PlaybackIdentity>,
    pub intent_digest: String,
    pub status: DeliveryStatus,
    pub owner_id: Uuid,
    pub dispatch_id: Option<Uuid>,
    pub lease_expires_at: DateTime<Utc>,
    pub observation_deadline_at: Option<DateTime<Utc>>,
    pub cancel_requested: bool,
    pub source_sample_rate: Option<u32>,
    pub source_channels: Option<u16>,
    pub offered_frames: u64,
    pub accepted_frames: u64,
    pub submitted_frames: Option<u64>,
    pub presented_frames: Option<u64>,
    pub end_accepted: bool,
    pub receipt_sequence: u64,
    #[schema(value_type = Option<serde_json::Value>)]
    pub progress: Option<alia_core::delivery::PlaybackProgress>,
    pub outcome: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub text: Option<TextDeliveryState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMedium {
    Audio,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TextDeliveryState {
    #[schema(value_type=Option<serde_json::Value>)]
    pub identity: Option<alia_core::text_delivery::TextIdentity>,
    pub offered_utf8_bytes: u64,
    pub offered_chunks: u64,
    pub end_offered: bool,
    #[schema(value_type=Option<serde_json::Value>)]
    pub progress: Option<alia_core::text_delivery::TextProgress>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTextDelivery {
    pub response_id: Uuid,
    pub output_key: String,
    /// Immutable representation intent. Generated body is stored in ordered chunks.
    pub content: Value,
    pub identity: Option<alia_core::text_delivery::TextIdentity>,
    pub owner_id: Uuid,
}

/// A successful CAS returned this permit to exactly one supervisor. It is not
/// reconstructible from a database read or retry after a lost commit ACK.
#[derive(Debug)]
pub struct DispatchPermit {
    pub(crate) delivery_id: Uuid,
    pub(crate) owner_id: Uuid,
    pub(crate) dispatch_id: Uuid,
}

impl DispatchPermit {
    pub fn delivery_id(&self) -> Uuid {
        self.delivery_id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeliveryEvent {
    pub event_id: i64,
    pub response_id: Uuid,
    pub delivery_id: Option<Uuid>,
    pub kind: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    #[error("delivery is not authorized: {0}")]
    Unauthorized(String),
    #[error("response or delivery not found")]
    NotFound,
    #[error("delivery idempotency key belongs to a different intent")]
    IdempotencyConflict,
    #[error("invalid delivery transition: {0}")]
    InvalidTransition(String),
    #[error("invalid delivery evidence: {0}")]
    InvalidEvidence(String),
    #[error("delivery capacity exceeded")]
    CapacityExceeded,
    #[error("invalid delivery input: {0}")]
    InvalidInput(String),
    #[error("delivery database error: {0}")]
    Database(#[from] sea_orm::DbErr),
    #[error("invalid delivery data: {0}")]
    Data(#[from] serde_json::Error),
}

pub type DeliveryResult<T> = Result<T, DeliveryError>;
