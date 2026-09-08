use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const MAX_PENDING: u64 = 256;
pub const MAX_PROPOSED_PER_USER: u64 = 32;
pub const MAX_ARGUMENT_BYTES: usize = 16 * 1024;
pub const MAX_RECEIPT_BYTES: usize = 64 * 1024;
pub const MAX_EXECUTION_TIMEOUT: Duration = Duration::from_secs(300);
pub const APPROVAL_VALIDITY: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EffectKey {
    pub user_id: Uuid,
    pub request_id: Uuid,
    pub invocation_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEffect {
    pub key: EffectKey,
    pub model_call_id: String,
    pub source_received_at: DateTime<Utc>,
    pub source_event_id: Option<Uuid>,
    pub source: String,
    pub conversation_id: Option<Uuid>,
    pub channel_id: Option<String>,
    pub tool_name: String,
    pub arguments: Value,
    pub tool_digest: String,
    pub policy_digest: String,
    pub requires_confirmation: bool,
    pub required_role: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectStatus {
    Proposed,
    Approved,
    Dispatched,
    Succeeded,
    Failed,
    Unknown,
    Cancelled,
}

impl EffectStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Unknown | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Automatic,
    Explicit,
}

#[derive(Debug, Clone)]
pub struct Approval {
    pub actor_id: Uuid,
    pub kind: ApprovalKind,
    pub expected_intent_digest: String,
}

#[derive(Debug, Clone)]
pub struct ClaimRequest {
    pub effect_id: Uuid,
    pub worker_id: Uuid,
    pub expected_tool_digest: String,
    pub expected_policy_digest: String,
    pub execution_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Completion {
    Succeeded { receipt: Value },
    Failed { evidence: Value },
    Unknown { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EffectRecord {
    pub effect_id: Uuid,
    pub key: EffectKey,
    pub model_call_id: String,
    /// First trusted source observation; retransmission never replaces it.
    pub source_received_at: DateTime<Utc>,
    pub source_event_id: Option<Uuid>,
    pub source: String,
    pub conversation_id: Option<Uuid>,
    pub channel_id: Option<String>,
    pub tool_name: String,
    pub arguments: Value,
    pub tool_digest: String,
    pub policy_digest: String,
    /// Snapshot of the database-owned catalog policy, checked under row lock.
    pub catalog_digest: String,
    pub intent_digest: String,
    pub requires_confirmation: bool,
    pub required_role: Option<String>,
    pub status: EffectStatus,
    pub approved_by: Option<Uuid>,
    pub approval_kind: Option<ApprovalKind>,
    pub approved_at: Option<DateTime<Utc>>,
    pub approval_expires_at: Option<DateTime<Utc>>,
    pub claim_id: Option<Uuid>,
    pub worker_id: Option<Uuid>,
    pub deadline_at: Option<DateTime<Utc>>,
    pub completion: Option<Completion>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ExecutionClaim {
    pub effect: EffectRecord,
    pub claim_id: Uuid,
    pub deadline_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("execution identity is not linked to an account")]
    IdentityUnlinked,
    #[error("execution is not authorized: {0}")]
    Unauthorized(String),
    #[error("effect not found")]
    NotFound,
    #[error("idempotency key belongs to a different intent")]
    IdempotencyConflict,
    #[error("invalid effect transition: {0}")]
    InvalidTransition(String),
    #[error("execution capacity exceeded")]
    CapacityExceeded,
    #[error("tool or policy version changed")]
    VersionChanged,
    #[error("invalid execution input: {0}")]
    InvalidInput(String),
    #[error("conflicting completion for this claim")]
    CompletionConflict,
    #[error("execution database error: {0}")]
    Database(#[from] sea_orm::DbErr),
    #[error("invalid execution data: {0}")]
    Data(#[from] serde_json::Error),
}

pub type ExecutionResult<T> = Result<T, ExecutionError>;
