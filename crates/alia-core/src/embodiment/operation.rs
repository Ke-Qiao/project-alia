//! Durable physical work is separate from conversational response delivery.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Submitting,
    Accepted,
    Running,
    Succeeded,
    Failed,
    Cancelling,
    Cancelled,
    Unknown,
}

impl OperationStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    pub fn allows(self, next: Self) -> bool {
        self == next
            || match self {
                Self::Submitting => next != Self::Cancelling,
                Self::Accepted => next != Self::Submitting,
                Self::Running => !matches!(next, Self::Accepted | Self::Submitting),
                Self::Cancelling => matches!(
                    next,
                    Self::Succeeded | Self::Failed | Self::Cancelled | Self::Unknown
                ),
                Self::Unknown => next != Self::Submitting,
                Self::Succeeded | Self::Failed | Self::Cancelled => false,
            }
    }
}

/// Every command and observation carries the original device incarnation and
/// host-approved implementation identity. An ID alone is not an authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationIdentity {
    pub operation_id: Uuid,
    pub owner_id: Uuid,
    pub device_id: Uuid,
    pub boot_id: Uuid,
    pub adapter_id: String,
    pub revision: String,
    pub capability: String,
    pub capability_pin: String,
}

/// Positive evidence that this operation no longer controls its resources.
/// A cancel acknowledgement is never sufficient evidence of a physical stop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoppedEvidence {
    pub observed_at_ms: i64,
    pub evidence: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationFeedback {
    pub identity: OperationIdentity,
    /// Strictly increasing across start, observe and cancel for this operation.
    pub sequence: u64,
    pub observed_at_ms: i64,
    pub status: OperationStatus,
    pub payload: Value,
    pub stopped: Option<StoppedEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationRecord {
    pub identity: OperationIdentity,
    pub start_effect_id: Uuid,
    /// Most recent separately authorized cancellation; the original start
    /// effect and its receipt remain immutable.
    pub cancel_effect_id: Option<Uuid>,
    pub goal: Value,
    pub resources: Vec<String>,
    pub status: OperationStatus,
    /// Uncertainty never resets the adapter-confirmed transition graph.
    pub last_confirmed_status: Option<OperationStatus>,
    pub last_sequence: u64,
    pub last_feedback_at_ms: Option<i64>,
    pub feedback: Value,
    pub stopped: Option<StoppedEvidence>,
    pub uncertainty: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}
