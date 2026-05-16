use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use serde::Serialize;

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
    },
    AudioInput {
        channel_id: String,
        user_id: String,
        audio: AudioPayload,
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

    /// Best-effort channel control hook used by channels that can stop active playback.
    async fn stop_current_playback(
        &self,
        _target: &ChannelTarget,
        _reason: &str,
    ) -> anyhow::Result<()> {
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
