use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{OperationFeedback, OperationIdentity};

/// Trusted bootstrap configuration, never model-supplied revision selection.
/// The principal is an explicit device grant; registering code alone is not one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterDescriptor {
    pub owner_id: Uuid,
    pub device_id: Uuid,
    pub boot_id: Uuid,
    /// Portable unique ID (1..32 ASCII letters/digits/underscores).
    pub adapter_id: String,
    pub revision: String,
    pub capability: String,
    pub capability_pin: String,
    pub goal_schema: Value,
    /// Device-local exclusive resource names, fixed by the trusted adapter.
    pub resources: Vec<String>,
}

impl AdapterDescriptor {
    pub fn start_tool_name(&self) -> String {
        format!("embodiment_{}_start", self.adapter_id)
    }
    pub fn cancel_tool_name(&self) -> String {
        format!("embodiment_{}_cancel", self.adapter_id)
    }
    pub(crate) fn identity(&self, operation_id: Uuid) -> OperationIdentity {
        OperationIdentity {
            operation_id,
            owner_id: self.owner_id,
            device_id: self.device_id,
            boot_id: self.boot_id,
            adapter_id: self.adapter_id.clone(),
            revision: self.revision.clone(),
            capability: self.capability.clone(),
            capability_pin: self.capability_pin.clone(),
        }
    }
    pub(crate) fn matches(&self, identity: &OperationIdentity) -> bool {
        self.identity(identity.operation_id) == *identity
    }
}

#[derive(Debug, Clone)]
pub struct OperationRequest {
    pub identity: OperationIdentity,
    pub goal: Value,
}

/// Implementations must use operation_id as the external idempotency key and
/// query that exact operation. They must not translate an acknowledgement into
/// completion, or cancellation into stopped evidence. Calls remain owned until
/// they return, even after the host has recorded a timeout as Unknown.
/// A terminal stopped observation must fence any delayed command for this ID:
/// retain a terminal tombstone and never let a late start reactivate it. A
/// not-found query alone proves neither completion nor stop. Database call
/// ordering is not a device fence when a database session or host is lost.
#[async_trait]
pub trait PhysicalAdapter: Send + Sync {
    fn descriptor(&self) -> AdapterDescriptor;
    async fn start(&self, request: OperationRequest) -> anyhow::Result<OperationFeedback>;
    async fn observe(&self, identity: OperationIdentity) -> anyhow::Result<OperationFeedback>;
    async fn cancel(&self, identity: OperationIdentity) -> anyhow::Result<OperationFeedback>;
}
