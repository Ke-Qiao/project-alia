use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Immutable origin of voice output. Never fill it from a channel's current
/// session when old asynchronous work finally reaches the transport adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VoiceScope {
    pub device_boot_id: Uuid,
    pub session_id: Uuid,
    pub generation_epoch: u64,
}

impl VoiceScope {
    pub fn is_valid(&self) -> bool {
        !self.device_boot_id.is_nil() && !self.session_id.is_nil() && self.generation_epoch > 0
    }
}
