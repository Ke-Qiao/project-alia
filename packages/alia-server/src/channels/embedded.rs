use std::sync::Arc;
use std::time::Duration;

use alia_transport::{
    DEFAULT_GRPC_AUTH_TTL, bind_payload, current_unix_timestamp, discover_service_endpoint,
    handoff_completion_payload, handoff_intent_payload, mint_auth_token, sign_payload,
};
use async_trait::async_trait;
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::SigningKey;
use tokio::sync::RwLock;
use tonic::Request;
use tonic::transport::Channel;

use alia_proto::audio::audio_stream_client::AudioStreamClient;
use alia_proto::audio::{AudioChunk, AudioEncoding as ProtoAudioEncoding, PlayAudioRequest};
use alia_proto::binding::device_binding_client::DeviceBindingClient;
use alia_proto::plugin::plugin_manager_client::PluginManagerClient;
use alia_proto::process::process_manager_client::ProcessManagerClient;

use crate::auth::load_server_keypair;
use crate::event::EventBus;
use crate::module::ModuleLifecycle;
use crate::module::lifecycle::{ModuleContext, ModuleHealth};

use super::adapter::{
    AudioFormat, AudioPayload, ChannelAction, ChannelAdapter, ChannelCapabilities, ChannelTarget,
};

/// Configuration for a single EmbeddedChannel instance.
#[derive(Debug, Clone)]
pub struct EmbeddedChannelConfig {
    pub id: String,
    pub endpoint: String,
    pub discovery_service_name: String,
    pub discovery_timeout_sec: u64,
    pub reconnect_interval_sec: u64,
    pub reconnect_max_backoff_sec: u64,
    pub grpc_timeout_sec: u64,
}

#[derive(Debug, Clone)]
enum ConnectionStatus {
    Connected,
    Disconnected {
        since: DateTime<Utc>,
        reason: String,
    },
    Reconnecting {
        attempt: u32,
    },
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
    signing_key: Option<SigningKey>,
    server_public_key: Vec<u8>,
    default_server_id: String,
    default_server_name: String,
    default_server_endpoint: String,
}

impl EmbeddedChannel {
    pub fn new(config: EmbeddedChannelConfig, event_bus: EventBus) -> Self {
        let status = if config.endpoint.is_empty() {
            ConnectionStatus::Disconnected {
                since: Utc::now(),
                reason: "awaiting embedded discovery".into(),
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
            signing_key: None,
            server_public_key: Vec::new(),
            default_server_id: "alia-server".to_string(),
            default_server_name: "alia-server".to_string(),
            default_server_endpoint: String::new(),
        }
    }

    async fn resolve_endpoint(&self) -> anyhow::Result<Option<String>> {
        if !self.config.endpoint.is_empty() {
            return Ok(Some(self.config.endpoint.clone()));
        }

        discover_service_endpoint(
            &self.config.discovery_service_name,
            Duration::from_secs(self.config.discovery_timeout_sec),
        )
        .await
    }

    /// Establish gRPC connection to alia-embedded.
    async fn connect(&self) -> anyhow::Result<()> {
        let Some(endpoint) = self.resolve_endpoint().await? else {
            *self.status.write().await = ConnectionStatus::Disconnected {
                since: Utc::now(),
                reason: format!(
                    "no embedded endpoint discovered for {}",
                    self.config.discovery_service_name
                ),
            };
            return Ok(());
        };

        let mut attempts = 0u32;
        loop {
            attempts += 1;
            *self.status.write().await = ConnectionStatus::Reconnecting { attempt: attempts };

            match Channel::from_shared(endpoint.clone())
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
                    *self.binding_client.write().await = Some(DeviceBindingClient::new(channel));
                    *self.status.write().await = ConnectionStatus::Connected;
                    tracing::info!(
                        endpoint = %endpoint,
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
                            endpoint = %endpoint,
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

    fn auth_request<T>(&self, message: T) -> anyhow::Result<Request<T>> {
        let signing_key = self
            .signing_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("server signing key not initialized"))?;
        let token = mint_auth_token(
            signing_key,
            &self.default_server_id,
            "embedded:control",
            DEFAULT_GRPC_AUTH_TTL,
        )?;
        let mut request = Request::new(message);
        let header = format!("Bearer {token}")
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid authorization metadata: {e}"))?;
        request.metadata_mut().insert("authorization", header);
        Ok(request)
    }
}

#[async_trait]
impl ModuleLifecycle for EmbeddedChannel {
    fn name(&self) -> &'static str {
        "embedded-channel"
    }

    async fn init(&mut self, _ctx: &ModuleContext) -> anyhow::Result<()> {
        let keypair = load_server_keypair(&_ctx.config.auth.server_keypair_path)?;
        self.server_public_key = keypair.public_key_bytes().to_vec();
        self.signing_key = Some(keypair.signing_key().clone());
        self.default_server_id = _ctx.config.server.id.clone();
        self.default_server_name = format!("alia-server@{}", _ctx.config.server.host);
        self.default_server_endpoint = format!(
            "http://{}:{}",
            _ctx.config.server.host, _ctx.config.server.port
        );
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
        let mut response_stream = client
            .play_audio(self.auth_request(request)?)
            .await?
            .into_inner();

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
            tracing::trace!(seq = ack.sequence, "play_audio ack from Pi");
        }

        // Also send raw chunk via UploadAudio for the actual data delivery
        let mut upload_client = self.audio_client().await?;
        let upload_stream = tokio_stream::once(chunk);
        upload_client
            .upload_audio(self.auth_request(upload_stream)?)
            .await?;

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
                    .start_process(self.auth_request(alia_proto::process::StartProcessRequest {
                        name: name.to_string(),
                    })?)
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
                    .stop_process(self.auth_request(alia_proto::process::StopProcessRequest {
                        name: name.to_string(),
                        force,
                    })?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "message": resp.message,
                }))
            }
            "send_plugin_command" => {
                let plugin = action.params["plugin"].as_str().ok_or_else(|| {
                    anyhow::anyhow!("send_plugin_command requires 'plugin' param")
                })?;
                let command_action = action.params["action"].as_str().unwrap_or("");
                let payload = action.params["payload"].as_str().unwrap_or("{}");
                let mut client = self.plugin_client().await?;
                let resp = client
                    .send_command(self.auth_request(alia_proto::plugin::PluginCommandRequest {
                        plugin_name: plugin.to_string(),
                        command_id: uuid::Uuid::new_v4().to_string(),
                        action: command_action.to_string(),
                        payload_json: payload.as_bytes().to_vec(),
                    })?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "message": resp.message,
                }))
            }
            "binding_status" => {
                let mut client = self.binding_client().await?;
                let resp = client
                    .get_binding_status(self.auth_request(())?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "is_bound": resp.is_bound,
                    "bound_server_id": resp.bound_server_id,
                    "bound_server_name": resp.bound_server_name,
                    "bound_since": resp.bound_since,
                    "state": resp.state,
                }))
            }
            "bind_device" => {
                let signing_key = self
                    .signing_key
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("server signing key not initialized"))?;
                let server_id = action.params["server_id"]
                    .as_str()
                    .unwrap_or(&self.default_server_id);
                let server_name = action.params["server_name"]
                    .as_str()
                    .unwrap_or(&self.default_server_name);
                let server_endpoint = action.params["server_endpoint"]
                    .as_str()
                    .unwrap_or(&self.default_server_endpoint);
                let issued_at = current_unix_timestamp()?;
                let payload = bind_payload(
                    server_id,
                    server_name,
                    server_endpoint,
                    &self.server_public_key,
                    issued_at,
                );
                let auth_token = sign_payload(signing_key, &payload);

                let mut client = self.binding_client().await?;
                let resp = client
                    .bind(alia_proto::binding::BindRequest {
                        server_id: server_id.to_string(),
                        server_name: server_name.to_string(),
                        server_endpoint: server_endpoint.to_string(),
                        auth_token,
                        server_public_key: self.server_public_key.clone(),
                        issued_at,
                    })
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "device_id": resp.device_id,
                    "device_name": resp.device_name,
                    "device_public_key": base64::engine::general_purpose::STANDARD.encode(resp.device_public_key),
                }))
            }
            "unbind_device" => {
                let server_id = action.params["server_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("unbind_device requires 'server_id' param"))?;
                let mut client = self.binding_client().await?;
                let resp = client
                    .unbind(self.auth_request(alia_proto::binding::UnbindRequest {
                        server_id: server_id.to_string(),
                    })?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "message": resp.message,
                }))
            }
            "prepare_handoff" => {
                let signing_key = self
                    .signing_key
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("server signing key not initialized"))?;
                let current_server_id =
                    action.params["current_server_id"].as_str().ok_or_else(|| {
                        anyhow::anyhow!("prepare_handoff requires 'current_server_id' param")
                    })?;
                let new_server_id = action.params["new_server_id"].as_str().ok_or_else(|| {
                    anyhow::anyhow!("prepare_handoff requires 'new_server_id' param")
                })?;
                let new_server_name =
                    action.params["new_server_name"].as_str().ok_or_else(|| {
                        anyhow::anyhow!("prepare_handoff requires 'new_server_name' param")
                    })?;
                let new_server_endpoint = action.params["new_server_endpoint"]
                    .as_str()
                    .ok_or_else(|| {
                        anyhow::anyhow!("prepare_handoff requires 'new_server_endpoint' param")
                    })?;
                let new_server_public_key_b64 = action.params["new_server_public_key"]
                    .as_str()
                    .ok_or_else(|| {
                        anyhow::anyhow!("prepare_handoff requires 'new_server_public_key' param")
                    })?;
                let new_server_public_key =
                    base64::engine::general_purpose::STANDARD.decode(new_server_public_key_b64)?;
                let issued_at = current_unix_timestamp()?;
                let payload = handoff_intent_payload(
                    current_server_id,
                    new_server_id,
                    new_server_name,
                    new_server_endpoint,
                    &new_server_public_key,
                    issued_at,
                );
                let authorization = sign_payload(signing_key, &payload);

                let mut client = self.binding_client().await?;
                let resp = client
                    .prepare_handoff(self.auth_request(alia_proto::binding::HandoffRequest {
                        current_server_id: current_server_id.to_string(),
                        new_server_id: new_server_id.to_string(),
                        new_server_endpoint: new_server_endpoint.to_string(),
                        new_server_name: new_server_name.to_string(),
                        new_server_public_key,
                        authorization,
                        issued_at,
                    })?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "token": base64::engine::general_purpose::STANDARD.encode(resp.token),
                    "device_id": resp.device_id,
                    "expires_at": resp.expires_at,
                }))
            }
            "complete_handoff" => {
                let signing_key = self
                    .signing_key
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("server signing key not initialized"))?;
                let token_b64 = action.params["token"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("complete_handoff requires 'token' param"))?;
                let token = base64::engine::general_purpose::STANDARD.decode(token_b64)?;
                let new_server_id = action.params["new_server_id"].as_str().ok_or_else(|| {
                    anyhow::anyhow!("complete_handoff requires 'new_server_id' param")
                })?;
                let new_server_name = action.params["new_server_name"]
                    .as_str()
                    .unwrap_or(&self.default_server_name);
                let new_server_endpoint = action.params["new_server_endpoint"]
                    .as_str()
                    .unwrap_or(&self.default_server_endpoint);
                let issued_at = current_unix_timestamp()?;
                let payload = handoff_completion_payload(
                    &token,
                    new_server_id,
                    new_server_name,
                    new_server_endpoint,
                    &self.server_public_key,
                    issued_at,
                );
                let authorization = sign_payload(signing_key, &payload);

                let mut client = self.binding_client().await?;
                let resp = client
                    .complete_handoff(self.auth_request(
                        alia_proto::binding::CompleteHandoffRequest {
                            token,
                            new_server_id: new_server_id.to_string(),
                            new_server_name: new_server_name.to_string(),
                            new_server_endpoint: new_server_endpoint.to_string(),
                            new_server_public_key: self.server_public_key.clone(),
                            authorization,
                            issued_at,
                        },
                    )?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "device_id": resp.device_id,
                    "device_name": resp.device_name,
                    "device_public_key": base64::engine::general_purpose::STANDARD.encode(resp.device_public_key),
                }))
            }
            _ => Err(anyhow::anyhow!(
                "action '{}' not supported by embedded channel",
                action.name
            )),
        }
    }
}
