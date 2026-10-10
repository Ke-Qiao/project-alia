use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::modules::delivery::transport::DeliveryTransport;
use crate::modules::platform::lifecycle::ModuleLifecycle;

/// Channel capabilities — declares what I/O a channel supports.
///
#[derive(Debug, Clone, Default)]
pub struct ChannelCapabilities {
    pub text: bool,
    pub voice_input: bool,
    pub voice_output: bool,
    pub images: bool,
    pub rich_messages: bool,
    /// Render text incrementally using the owned text delivery contract.
    pub streaming: bool,
    pub sensors: bool,
    pub hardware_control: bool,
    pub presence: bool,
}

/// A named action to execute on a channel (hardware control, etc.).
#[derive(Debug, Clone)]
pub struct ChannelAction {
    pub name: String,
    pub params: serde_json::Value,
}

/// Channel status.
#[derive(Debug, Clone, Serialize)]
pub enum ChannelStatus {
    Connected,
    Disconnected { reason: String },
    Reconnecting { attempt: u32 },
}

/// Inbound event from a channel into the AI pipeline.
#[derive(Debug, Clone, Serialize)]
pub enum ChannelEvent {
    TextMessage {
        channel_id: String,
        user_id: String,
        text: String,
        /// Server receive time, fixed before authentication/processing waits.
        source_received_at: DateTime<Utc>,
        /// Database-ordered privacy version captured at trusted source admission.
        source_privacy_epoch: i64,
        source_relationship_epoch: i64,
    },
    SensorData {
        channel_id: String,
        sensor_type: String,
        payload: serde_json::Value,
    },
    StatusChange {
        channel_id: String,
        status: ChannelStatus,
    },
}

/// Channel adapter trait — each channel is a path for user interaction.
///
/// Unlike module slots (single-instance), channels support multiple concurrent instances.
/// A channel only handles I/O conversion; business logic lives in the AI pipeline.
#[async_trait]
pub trait ChannelAdapter: ModuleLifecycle + DeliveryTransport {
    /// Channel type identifier (e.g. "embedded", "cli").
    fn channel_type(&self) -> &'static str;

    /// Revalidate the account selected by this channel. Custom ingress adapters
    /// must supply their trusted account lookup; static configuration alone is insufficient.
    async fn authenticated_owner(&self) -> anyhow::Result<uuid::Uuid> {
        anyhow::bail!("channel does not provide account verification")
    }

    /// Admit a source against the authenticated owner's database privacy epoch.
    /// Production adapters must validate the active account under the same
    /// database ordering as erasure. Never derive this value from device input
    /// or capture it again when dispatching an already admitted source.
    async fn authenticated_source_epochs(
        &self,
    ) -> anyhow::Result<crate::modules::memory::source::SourceEpochs> {
        anyhow::bail!("channel does not provide trusted source admission")
    }

    /// Declare what this channel supports.
    fn capabilities(&self) -> ChannelCapabilities;

    /// A voice fence has a specific immutable origin and minimum epoch. It
    /// must never be implemented as an unscoped "stop whatever plays now".
    async fn fence_voice(
        &self,
        _scope: alia_core::delivery::VoiceScope,
        _close_session: bool,
        _reason: &str,
    ) -> anyhow::Result<()> {
        if self.capabilities().voice_output {
            anyhow::bail!("channel does not support scoped voice playback control");
        }
        Ok(())
    }

    /// Execute a channel-specific action (e.g. hardware control).
    async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
        Err(anyhow::anyhow!("action '{}' not supported", action.name))
    }
}
