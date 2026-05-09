use async_trait::async_trait;
use serde::Serialize;

use crate::module::ModuleLifecycle;

/// Channel capabilities — declares what I/O a channel supports.
/// AI Pipeline uses these to decide processing strategy.
#[derive(Debug, Clone, Default)]
pub struct ChannelCapabilities {
    pub text: bool,
    pub voice_input: bool,
    pub voice_output: bool,
    pub images: bool,
    pub rich_messages: bool,
    pub streaming: bool,
    pub sensors: bool,
    pub hardware_control: bool,
    pub presence: bool,
}

/// Identifies a target within a channel (user / device / chat).
#[derive(Debug, Clone)]
pub struct ChannelTarget {
    pub user_id: String,
}

/// A named action to execute on a channel (hardware control, etc.).
#[derive(Debug, Clone)]
pub struct ChannelAction {
    pub name: String,
    pub params: serde_json::Value,
}

/// Audio payload exchanged between channels and the AI pipeline.
#[derive(Debug, Clone, Serialize)]
pub struct AudioPayload {
    pub data: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u16,
    pub format: AudioFormat,
}

#[derive(Debug, Clone, Serialize)]
pub enum AudioFormat {
    Pcm16,
    Opus,
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
    },
    AudioInput {
        channel_id: String,
        user_id: String,
        audio: AudioPayload,
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

/// Outbound response from the AI pipeline to a channel.
#[derive(Debug, Clone)]
pub struct ChannelResponse {
    pub channel_id: String,
    pub target: ChannelTarget,
    pub text: Option<String>,
    pub audio: Option<AudioPayload>,
    pub actions: Vec<ChannelAction>,
}

/// Channel adapter trait — each channel is a path for user interaction.
///
/// Unlike module slots (single-instance), channels support multiple concurrent instances.
/// A channel only handles I/O conversion; business logic lives in the AI pipeline.
#[async_trait]
pub trait ChannelAdapter: ModuleLifecycle {
    /// Channel type identifier (e.g. "embedded", "cli").
    fn channel_type(&self) -> &'static str;

    /// Instance ID (unique across all channels).
    fn channel_id(&self) -> &str;

    /// Declare what this channel supports.
    fn capabilities(&self) -> ChannelCapabilities;

    /// Send a text message to the channel.
    async fn send_message(&self, target: &ChannelTarget, msg: &str) -> anyhow::Result<()>;

    /// Send audio (TTS output) to the channel.
    async fn send_audio(
        &self,
        _target: &ChannelTarget,
        _audio: AudioPayload,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "audio not supported by channel '{}'",
            self.channel_type()
        ))
    }

    /// Execute a channel-specific action (e.g. hardware control).
    async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
        Err(anyhow::anyhow!("action '{}' not supported", action.name))
    }
}
