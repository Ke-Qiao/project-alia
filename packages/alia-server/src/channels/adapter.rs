use std::pin::Pin;
use std::sync::Arc;

use alia_core::delivery::{ObservationEnd, PlaybackIdentity, PlaybackProgress};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::module::ModuleLifecycle;

/// Channel capabilities — declares what I/O a channel supports.
///
/// The pipeline inspects these flags to decide how to deliver responses:
/// - `streaming = true`: the pipeline will call `begin_stream / send_stream_chunk
///   / end_stream` for LLM responses, delivering tokens incrementally.
/// - `streaming = false` (default): the pipeline will call `send_message` with
///   the complete response after the LLM finishes generating.
#[derive(Debug, Clone, Default)]
pub struct ChannelCapabilities {
    pub text: bool,
    pub voice_input: bool,
    pub voice_output: bool,
    pub images: bool,
    pub rich_messages: bool,
    /// When `true`, the pipeline delivers LLM tokens incrementally via the
    /// `begin_stream / send_stream_chunk / end_stream` methods instead of
    /// waiting for the full response and calling `send_message`.
    pub streaming: bool,
    /// When `true`, the channel can consume TTS audio chunks incrementally.
    /// Channels that leave this false use a safe full-buffer fallback.
    pub audio_streaming: bool,
    pub sensors: bool,
    pub hardware_control: bool,
    pub presence: bool,
    /// When `true`, the channel can accept generic conversation control
    /// actions such as ending local continuous/follow-up listening.
    pub conversation_control: bool,
}

/// Channel-local voice turn interruption behavior.
///
/// Channels opt in by overriding [`ChannelAdapter::interrupt_config`].  This is
/// intentionally not global because plugin channels may have different turn,
/// playback, or safety semantics.
#[derive(Debug, Clone)]
pub struct ChannelInterruptConfig {
    pub enabled: bool,
    pub cancel_streaming_tts_on_interrupt: bool,
    pub discard_interrupted_turn_history: bool,
}

impl Default for ChannelInterruptConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cancel_streaming_tts_on_interrupt: true,
            discard_interrupted_turn_history: true,
        }
    }
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
    pub data: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u16,
    pub format: AudioFormat,
}

pub type AudioPayloadStream =
    Pin<Box<dyn Stream<Item = anyhow::Result<AudioPayload>> + Send + 'static>>;

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
    /// Legacy transports without evidence leave the entire progress absent.
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
    },
    AudioInput {
        channel_id: String,
        user_id: String,
        audio: AudioPayload,
        /// Server receive time; never sourced from a device-provided clock.
        source_received_at: DateTime<Utc>,
        /// Immutable across media buffering, transcription and pipeline queues.
        source_privacy_epoch: i64,
    },
    InterruptActiveTurn {
        channel_id: String,
        user_id: String,
        reason: String,
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
    async fn authenticated_source_epoch(&self) -> anyhow::Result<i64> {
        anyhow::bail!("channel does not provide trusted source admission")
    }

    /// Declare what this channel supports.
    fn capabilities(&self) -> ChannelCapabilities;

    /// Declare channel-local interrupt behavior.
    fn interrupt_config(&self) -> ChannelInterruptConfig {
        ChannelInterruptConfig::default()
    }

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

    /// Send a stream of audio chunks to the channel.
    ///
    /// The default implementation preserves correctness by buffering chunks and
    /// delegating to `send_audio`. Low-latency channels should override this to
    /// apply backpressure and start playback as soon as enough audio is ready.
    async fn send_audio_stream(
        &self,
        target: &ChannelTarget,
        mut audio: AudioPayloadStream,
    ) -> anyhow::Result<()> {
        let Some(first) = audio.next().await else {
            return Ok(());
        };
        let mut combined = first?;

        while let Some(next) = audio.next().await {
            let next = next?;
            if next.sample_rate != combined.sample_rate
                || next.channels != combined.channels
                || next.format != combined.format
            {
                anyhow::bail!(
                    "audio stream changed format from {:?}/{}Hz/{}ch to {:?}/{}Hz/{}ch",
                    combined.format,
                    combined.sample_rate,
                    combined.channels,
                    next.format,
                    next.sample_rate,
                    next.channels
                );
            }
            combined.data.extend(next.data);
        }

        self.send_audio(target, combined).await
    }

    /// Reserve only local sequence/identity. The service supplies the UUID and
    /// persists the resulting immutable intent before any external I/O.
    fn reserve_playback(
        &self,
        target: &ChannelTarget,
        _playback_id: uuid::Uuid,
    ) -> anyhow::Result<Option<PlaybackIdentity>> {
        anyhow::ensure!(
            target.voice.is_none(),
            "channel does not support scoped playback reservation"
        );
        Ok(None)
    }

    /// Owned by Delivery supervision. Dropping a pipeline waiter does not drop
    /// this future. Legacy acceptance cannot produce submitted/presented counts.
    async fn deliver_audio(
        &self,
        target: &ChannelTarget,
        identity: Option<PlaybackIdentity>,
        audio: AudioPayloadStream,
        _observer: Arc<dyn PlaybackObserver>,
        cancel: CancellationToken,
    ) -> anyhow::Result<AudioDeliveryReport> {
        anyhow::ensure!(
            identity.is_none() && target.voice.is_none(),
            "scoped delivery requires a protocol adapter"
        );
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Ok(AudioDeliveryReport::unobserved("legacy delivery cancelled without scoped device evidence")),
            result = self.send_audio_stream(target, audio) => {
                result?;
                Ok(AudioDeliveryReport::unobserved("legacy transport returned without device progress evidence"))
            }
        }
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

    /// Best-effort channel control hook used by channels that can stop active playback.
    async fn stop_current_playback(
        &self,
        _target: &ChannelTarget,
        _reason: &str,
    ) -> anyhow::Result<()> {
        Ok(())
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

    /// Begin a streaming response. Called once before the first chunk.
    /// Only called when `capabilities().streaming` is `true`.
    /// Channels can use this to prepare the UI (e.g. clear a spinner, print a prefix).
    async fn begin_stream(&self, _target: &ChannelTarget) -> anyhow::Result<()> {
        Ok(())
    }

    /// Deliver a single streaming text chunk (token or small fragment).
    /// Only called when `capabilities().streaming` is `true`.
    /// Called repeatedly between `begin_stream` and `end_stream`.
    async fn send_stream_chunk(&self, _target: &ChannelTarget, _chunk: &str) -> anyhow::Result<()> {
        Ok(())
    }

    /// End a streaming response. Called after the last chunk.
    /// Only called when `capabilities().streaming` is `true`.
    /// `full_text` is the concatenated response (useful for logging).
    /// Default is a no-op because the pipeline does NOT call `send_message`
    /// after streaming — channels that streamed tokens already displayed them.
    async fn end_stream(&self, _target: &ChannelTarget, _full_text: &str) -> anyhow::Result<()> {
        Ok(())
    }

    /// Execute a channel-specific action (e.g. hardware control).
    async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
        Err(anyhow::anyhow!("action '{}' not supported", action.name))
    }
}
