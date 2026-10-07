use std::pin::Pin;
use std::sync::Arc;

use alia_core::delivery::{ObservationEnd, PlaybackIdentity, PlaybackProgress};
use alia_core::text_delivery::{TextChunk, TextFrame, TextIdentity, TextProgress};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::Stream;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::modules::platform::lifecycle::ModuleLifecycle;
pub use crate::providers::tts::audio_budget::AudioBuffer;

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
    /// When `true`, the channel can consume TTS audio chunks incrementally.
    /// Channels that leave this false receive one bounded synthesized payload.
    pub audio_streaming: bool,
    pub sensors: bool,
    pub hardware_control: bool,
    pub presence: bool,
}

/// Identifies a target within a channel (user / device / chat).
#[derive(Debug, Clone)]
pub struct ChannelTarget {
    pub user_id: String,
    pub voice: Option<alia_core::delivery::VoiceScope>,
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
    pub data: AudioBuffer,
    pub sample_rate: u32,
    pub channels: u16,
    pub format: AudioFormat,
}

pub type AudioPayloadStream =
    Pin<Box<dyn Stream<Item = anyhow::Result<AudioPayload>> + Send + 'static>>;

pub type TextPayloadStream =
    Pin<Box<dyn Stream<Item = anyhow::Result<TextFrame>> + Send + 'static>>;

#[async_trait]
pub trait TextObserver: Send + Sync {
    async fn start(&self) -> anyhow::Result<()>;
    async fn offered(&self, chunk: &TextChunk) -> anyhow::Result<()>;
    async fn end(&self, chunks: u64, utf8_bytes: u64) -> anyhow::Result<()>;
    async fn progress(&self, progress: TextProgress) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Serialize)]
pub struct TextDeliveryReport {
    pub progress: Option<TextProgress>,
    pub observation_end: ObservationEnd,
}
impl TextDeliveryReport {
    pub fn unobserved(reason: impl Into<String>) -> Self {
        Self {
            progress: None,
            observation_end: ObservationEnd::Unknown {
                reason: reason.into(),
            },
        }
    }
}

/// The transport awaits these durable writes before sending Start/Audio or
/// acknowledging progress. A database failure stops further media admission.
#[async_trait]
pub trait PlaybackObserver: Send + Sync {
    async fn start(&self, sample_rate_hz: u32, channels: u16) -> anyhow::Result<()>;
    async fn offered(&self, source_frames: u64) -> anyhow::Result<()>;
    async fn progress(&self, progress: PlaybackProgress) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioDeliveryReport {
    /// No received device evidence leaves progress absent; never infer completion.
    pub progress: Option<PlaybackProgress>,
    pub observation_end: ObservationEnd,
}

impl AudioDeliveryReport {
    pub fn unobserved(reason: impl Into<String>) -> Self {
        Self {
            progress: None,
            observation_end: ObservationEnd::Unknown {
                reason: reason.into(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum AudioFormat {
    Pcm16,
    Opus,
    Wav,
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
pub trait ChannelAdapter: ModuleLifecycle {
    /// Channel type identifier (e.g. "embedded", "cli").
    fn channel_type(&self) -> &'static str;

    /// Instance ID (unique across all channels).
    fn channel_id(&self) -> &str;

    /// Account selected by trusted server configuration, never by an audio frame.
    fn owner_user_id(&self) -> Option<uuid::Uuid> {
        None
    }

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

    /// Reserve a writer identity without external output. The service owns the
    /// UUID and commits it before the adapter may display any part of a stream.
    fn reserve_text(
        &self,
        _target: &ChannelTarget,
        _text_id: uuid::Uuid,
    ) -> anyhow::Result<TextIdentity> {
        anyhow::bail!("channel does not support text reservation")
    }

    /// Unsupported channels never fall back to identityless external writes.
    async fn deliver_text(
        &self,
        _target: &ChannelTarget,
        _identity: TextIdentity,
        _text: TextPayloadStream,
        _observer: Arc<dyn TextObserver>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<TextDeliveryReport> {
        anyhow::bail!("channel does not implement owned text delivery")
    }

    /// Exact writer cancellation closes this identity's admission and returns a
    /// checkpoint only after in-flight writes have finished. No global stop.
    async fn cancel_text(
        &self,
        _identity: TextIdentity,
        _reason: &str,
    ) -> anyhow::Result<TextProgress> {
        anyhow::bail!("channel does not support exact text cancellation")
    }

    /// Reserve only local sequence/identity. The service supplies the UUID and
    /// persists the resulting immutable intent before any external I/O.
    fn reserve_playback(
        &self,
        _target: &ChannelTarget,
        _playback_id: uuid::Uuid,
    ) -> anyhow::Result<PlaybackIdentity> {
        anyhow::bail!("channel does not support playback reservation")
    }

    /// Owned by Delivery supervision; no unscoped output fallback.
    async fn deliver_audio(
        &self,
        _target: &ChannelTarget,
        _identity: PlaybackIdentity,
        _audio: AudioPayloadStream,
        _observer: Arc<dyn PlaybackObserver>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<AudioDeliveryReport> {
        anyhow::bail!("channel does not implement owned audio delivery")
    }

    /// A precise playback fence must not stop a later playback in the same
    /// voice generation. Only final evidence after entered backend writes have
    /// settled can acknowledge it. A generic stop hook is insufficient.
    async fn cancel_playback(
        &self,
        _identity: PlaybackIdentity,
        _reason: &str,
    ) -> anyhow::Result<PlaybackProgress> {
        anyhow::bail!("channel does not support exact playback cancellation")
    }

    /// Register a canonical Response before any playback Start. The peer must
    /// retain this exact binding and report its actual native backend owner.
    async fn register_output(
        &self,
        _binding: alia_core::output_control::OutputBinding,
    ) -> anyhow::Result<alia_core::output_control::OutputRegistration> {
        anyhow::bail!("channel does not support playback v3 output registration")
    }

    async fn control_output(
        &self,
        _command: &alia_core::output_control::BoundControlCommand,
    ) -> anyhow::Result<alia_core::output_control::BoundControlReceipt> {
        anyhow::bail!("channel does not support reversible output control")
    }

    /// Read the original command's result; this must never apply a command.
    async fn observe_output_control(
        &self,
        _command: &alia_core::output_control::BoundControlCommand,
    ) -> anyhow::Result<alia_core::output_control::BoundControlReceipt> {
        anyhow::bail!("channel does not support output control observation")
    }

    /// Permanently retire this exact original owner and wait for admitted
    /// native writes to settle. A newer session or a generic stop is not proof.
    async fn retire_voice_output(
        &self,
        _request: &alia_core::output_control::OutputRetirementRequest,
    ) -> anyhow::Result<alia_core::output_control::OutputRetirementReceipt> {
        anyhow::bail!("channel does not support original output retirement")
    }

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
