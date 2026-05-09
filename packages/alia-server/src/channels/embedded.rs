use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tonic::transport::Channel;

use alia_proto::audio::audio_stream_client::AudioStreamClient;
use alia_proto::audio::{AudioChunk, AudioEncoding as ProtoAudioEncoding, PlayAudioRequest};
use alia_proto::binding::device_binding_client::DeviceBindingClient;
use alia_proto::plugin::plugin_manager_client::PluginManagerClient;
use alia_proto::process::process_manager_client::ProcessManagerClient;

use crate::event::EventBus;
use crate::module::lifecycle::{ModuleContext, ModuleHealth};
use crate::module::ModuleLifecycle;

use super::adapter::{
    AudioFormat, AudioPayload, ChannelAction, ChannelAdapter, ChannelCapabilities, ChannelTarget,
};

/// Configuration for a single EmbeddedChannel instance.
#[derive(Debug, Clone)]
pub struct EmbeddedChannelConfig {
    pub id: String,
    pub endpoint: String,
    pub reconnect_interval_sec: u64,
    pub reconnect_max_backoff_sec: u64,
    pub grpc_timeout_sec: u64,
}

#[derive(Debug, Clone)]
enum ConnectionStatus {
    Connected,
    Disconnected { since: DateTime<Utc>, reason: String },
    Reconnecting { attempt: u32 },
}

/// Channel adapter connecting to alia-embedded (Raspberry Pi) via gRPC.
///
/// Wraps the gRPC client logic that was previously in `embedded::EmbeddedClient`.
pub struct EmbeddedChannel {
    config: EmbeddedChannelConfig,
    event_bus: EventBus,
    status: Arc<RwLock<ConnectionStatus>>,
    audio_client: Arc<RwLock<Option<AudioStreamClient<Channel>>>>,
    process_client: Arc<RwLock<Option<ProcessManagerClient<Channel>>>>,
    plugin_client: Arc<RwLock<Option<PluginManagerClient<Channel>>>>,
    binding_client: Arc<RwLock<Option<DeviceBindingClient<Channel>>>>,
}

impl EmbeddedChannel {
    pub fn new(config: EmbeddedChannelConfig, event_bus: EventBus) -> Self {
        let status = if config.endpoint.is_empty() {
            ConnectionStatus::Disconnected {
                since: Utc::now(),
                reason: "no endpoint configured".into(),
            }
        } else {
            ConnectionStatus::Reconnecting { attempt: 0 }
        };

        Self {
            config,
            event_bus,
            status: Arc::new(RwLock::new(status)),
            audio_client: Arc::new(RwLock::new(None)),
            process_client: Arc::new(RwLock::new(None)),
            plugin_client: Arc::new(RwLock::new(None)),
            binding_client: Arc::new(RwLock::new(None)),
        }
    }

    /// Establish gRPC connection to alia-embedded.
    async fn connect(&self) -> anyhow::Result<()> {
        if self.config.endpoint.is_empty() {
            return Ok(());
        }

        let mut attempts = 0u32;
        loop {
            attempts += 1;
            *self.status.write().await = ConnectionStatus::Reconnecting { attempt: attempts };

            match Channel::from_shared(self.config.endpoint.clone())
                .map_err(|e| anyhow::anyhow!("invalid endpoint: {e}"))?
                .connect()
                .await
            {
                Ok(channel) => {
                    *self.audio_client.write().await =
                        Some(AudioStreamClient::new(channel.clone()));
                    *self.process_client.write().await =
                        Some(ProcessManagerClient::new(channel.clone()));
                    *self.plugin_client.write().await =
                        Some(PluginManagerClient::new(channel.clone()));
                    *self.binding_client.write().await =
                        Some(DeviceBindingClient::new(channel));
                    *self.status.write().await = ConnectionStatus::Connected;
                    tracing::info!(
                        endpoint = %self.config.endpoint,
                        channel_id = %self.config.id,
                        "connected to alia-embedded"
                    );
                    return Ok(());
                }
                Err(e) => {
                    if attempts >= 3 {
                        *self.status.write().await = ConnectionStatus::Disconnected {
                            since: Utc::now(),
                            reason: format!("failed after {attempts} attempts: {e}"),
                        };
                        tracing::warn!(
                            endpoint = %self.config.endpoint,
                            error = %e,
                            "failed to connect to alia-embedded, will retry lazily"
                        );
                        return Ok(());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500 * attempts as u64))
                        .await;
                }
            }
        }
    }

    /// Get a ProcessManagerClient, connecting lazily if needed.
    pub async fn process_client(&self) -> anyhow::Result<ProcessManagerClient<Channel>> {
        if self.process_client.read().await.is_none() {
            self.connect().await?;
        }
        self.process_client
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("embedded not connected"))
    }

    /// Get a PluginManagerClient, connecting lazily if needed.
    pub async fn plugin_client(&self) -> anyhow::Result<PluginManagerClient<Channel>> {
        if self.plugin_client.read().await.is_none() {
            self.connect().await?;
        }
        self.plugin_client
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("embedded not connected"))
    }

    /// Get a DeviceBindingClient, connecting lazily if needed.
    pub async fn binding_client(&self) -> anyhow::Result<DeviceBindingClient<Channel>> {
        if self.binding_client.read().await.is_none() {
            self.connect().await?;
        }
        self.binding_client
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("embedded not connected"))
    }

    /// Get an AudioStreamClient, connecting lazily if needed.
    pub async fn audio_client(&self) -> anyhow::Result<AudioStreamClient<Channel>> {
        if self.audio_client.read().await.is_none() {
            self.connect().await?;
        }
        self.audio_client
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("embedded not connected"))
    }
}

#[async_trait]
impl ModuleLifecycle for EmbeddedChannel {
    fn name(&self) -> &'static str {
        "embedded-channel"
    }

    async fn init(&mut self, _ctx: &ModuleContext) -> anyhow::Result<()> {
        self.connect().await?;
        Ok(())
    }

    async fn start(&self) -> anyhow::Result<()> {
        // TODO: start listening for inbound gRPC streams (audio upload, sensor data)
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        *self.status.write().await = ConnectionStatus::Disconnected {
            since: Utc::now(),
            reason: "shutdown".into(),
        };
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        match &*self.status.read().await {
            ConnectionStatus::Connected => ModuleHealth::Healthy,
            ConnectionStatus::Reconnecting { attempt } => {
                ModuleHealth::Degraded(format!("reconnecting (attempt {})", attempt))
            }
            ConnectionStatus::Disconnected { reason, .. } => {
                ModuleHealth::Unhealthy(reason.clone())
            }
        }
    }
}

#[async_trait]
impl ChannelAdapter for EmbeddedChannel {
    fn channel_type(&self) -> &'static str {
        "embedded"
    }

    fn channel_id(&self) -> &str {
        &self.config.id
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            text: true,
            voice_input: true,
            voice_output: true,
            images: false,
            rich_messages: false,
            streaming: true,
            sensors: true,
            hardware_control: true,
            presence: true,
        }
    }

    async fn send_message(&self, _target: &ChannelTarget, msg: &str) -> anyhow::Result<()> {
        // For the embedded channel, text messages are sent as TTS audio.
        // The AI pipeline calls send_audio() directly for voice_output channels,
        // but if text arrives here, we log it (Pi doesn't have a screen).
        tracing::debug!(
            channel_id = %self.config.id,
            msg_len = msg.len(),
            "embedded channel received text (no display on Pi)"
        );
        Ok(())
    }

    async fn send_audio(&self, _target: &ChannelTarget, audio: AudioPayload) -> anyhow::Result<()> {
        let mut client = self.audio_client().await?;

        let encoding = match audio.format {
            AudioFormat::Opus => ProtoAudioEncoding::Opus as i32,
            AudioFormat::Pcm16 => ProtoAudioEncoding::PcmS16le as i32,
        };

        let request = PlayAudioRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            source: "tts".into(),
            encoding,
            sample_rate: audio.sample_rate,
            interrupt_current: false,
        };

        // PlayAudio returns a server-stream of AudioChunks (ack from Pi).
        // We send the request with the audio data as a single chunk first,
        // then consume the response stream for confirmation.
        let mut response_stream = client.play_audio(request).await?.into_inner();

        // Send audio data via a separate UploadAudio call (Pi pulls from PlayAudio stream,
        // but the proto defines PlayAudio as server-stream for chunked playback confirmation).
        // For now, we chunk the audio into the UploadAudio client-stream:
        let chunk = AudioChunk {
            opus_data: audio.data,
            sequence: 0,
            timestamp_us: chrono::Utc::now().timestamp_micros() as u64,
            encoding,
        };

        // Drain the response stream (Pi sends back ack chunks as it plays)
        while let Some(ack) = response_stream.message().await? {
            tracing::trace!(
                seq = ack.sequence,
                "play_audio ack from Pi"
            );
        }

        // Also send raw chunk via UploadAudio for the actual data delivery
        let mut upload_client = self.audio_client().await?;
        let upload_stream = tokio_stream::once(chunk);
        upload_client.upload_audio(upload_stream).await?;

        Ok(())
    }

    async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
        // Route actions to the appropriate gRPC service on the Pi
        match action.name.as_str() {
            "start_process" => {
                let name = action.params["name"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("start_process requires 'name' param"))?;
                let mut client = self.process_client().await?;
                let resp = client
                    .start_process(alia_proto::process::StartProcessRequest {
                        name: name.to_string(),
                    })
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "message": resp.message,
                    "pid": resp.pid,
                }))
            }
            "stop_process" => {
                let name = action.params["name"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("stop_process requires 'name' param"))?;
                let force = action.params["force"].as_bool().unwrap_or(false);
                let mut client = self.process_client().await?;
                let resp = client
                    .stop_process(alia_proto::process::StopProcessRequest {
                        name: name.to_string(),
                        force,
                    })
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "message": resp.message,
                }))
            }
            "send_plugin_command" => {
                let plugin = action.params["plugin"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("send_plugin_command requires 'plugin' param"))?;
                let command_action = action.params["action"]
                    .as_str()
                    .unwrap_or("");
                let payload = action.params["payload"]
                    .as_str()
                    .unwrap_or("{}");
                let mut client = self.plugin_client().await?;
                let resp = client
                    .send_command(alia_proto::plugin::PluginCommandRequest {
                        plugin_name: plugin.to_string(),
                        command_id: uuid::Uuid::new_v4().to_string(),
                        action: command_action.to_string(),
                        payload_json: payload.as_bytes().to_vec(),
                    })
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "message": resp.message,
                }))
            }
            _ => Err(anyhow::anyhow!(
                "action '{}' not supported by embedded channel",
                action.name
            )),
        }
    }
}
