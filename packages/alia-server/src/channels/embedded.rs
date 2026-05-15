use std::sync::Arc;
use std::time::Duration;

use alia_transport::{
    DEFAULT_GRPC_AUTH_TTL, bind_payload, current_unix_timestamp, discover_service,
    discover_service_endpoint, handoff_completion_payload, handoff_intent_payload, mint_auth_token,
    sign_payload,
};
use async_trait::async_trait;
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::SigningKey;
use futures::StreamExt;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tonic::Request;
use tonic::transport::Channel;

use alia_proto::audio::audio_stream_client::AudioStreamClient;
use alia_proto::audio::{AudioChunk, AudioEncoding as ProtoAudioEncoding, PlayAudioRequest};
use alia_proto::binding::device_binding_client::DeviceBindingClient;
use alia_proto::plugin::plugin_manager_client::PluginManagerClient;
use alia_proto::process::process_manager_client::ProcessManagerClient;
use alia_proto::settings::embedded_settings_client::EmbeddedSettingsClient;

use crate::auth::load_server_keypair;
use crate::event::{EventBus, ServerEvent};
use crate::module::ModuleLifecycle;
use crate::module::lifecycle::{ModuleContext, ModuleHealth};

use super::adapter::{
    AudioFormat, AudioPayload, AudioPayloadStream, ChannelAction, ChannelAdapter,
    ChannelCapabilities, ChannelTarget,
};

const AUDIO_GRPC_CHUNK_BYTES: usize = 64 * 1024;
const AUDIO_STREAM_GRPC_CHUNK_BYTES: usize = 8 * 1024;
const AUDIO_STREAM_BACKPRESSURE_CHUNKS: usize = 8;
const GRPC_MESSAGE_LIMIT_BYTES: usize = 16 * 1024 * 1024;

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

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct DiscoveryCandidate {
    endpoint: String,
    device_id: Option<String>,
    device_name: Option<String>,
    status: String,
    provisioning_mode: bool,
    bind_required: bool,
    metadata: serde_json::Value,
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
    settings_client: Arc<RwLock<Option<EmbeddedSettingsClient<Channel>>>>,
    discovered_candidate: Arc<RwLock<Option<DiscoveryCandidate>>>,
    discovery_cancel: CancellationToken,
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
            settings_client: Arc::new(RwLock::new(None)),
            discovered_candidate: Arc::new(RwLock::new(None)),
            discovery_cancel: CancellationToken::new(),
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

    fn spawn_discovery_loop(&self) {
        let cancel = self.discovery_cancel.child_token();
        let status = self.status.clone();
        let discovered_candidate = self.discovered_candidate.clone();
        let event_bus = self.event_bus.clone();
        let channel_id = self.config.id.clone();
        let service_name = self.config.discovery_service_name.clone();
        let timeout = self.config.discovery_timeout_sec;

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(15));
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        let discovered = discover_service(&service_name, Duration::from_secs(timeout)).await;
                        let next_candidate = match discovered {
                            Ok(Some(service)) => {
                                let status_value = service.properties.get("status").cloned().unwrap_or_else(|| "unknown".to_string());
                                let provisioning_mode = service.properties.get("provisioning_mode").map(|value| value == "true").unwrap_or(false);
                                let candidate = DiscoveryCandidate {
                                    endpoint: service.endpoint,
                                    device_id: service.properties.get("id").cloned(),
                                    device_name: service.properties.get("name").cloned(),
                                    status: status_value.clone(),
                                    provisioning_mode,
                                    bind_required: status_value == "unbound",
                                    metadata: serde_json::to_value(service.properties).unwrap_or_else(|_| serde_json::json!({})),
                                };
                                Some(candidate)
                            }
                            Ok(None) => None,
                            Err(error) => {
                                tracing::warn!(channel_id = %channel_id, error = %error, "embedded discovery poll failed");
                                None
                            }
                        };

                        let mut stored = discovered_candidate.write().await;
                        if *stored != next_candidate {
                            match &next_candidate {
                                Some(candidate) => {
                                    tracing::info!(
                                        channel_id = %channel_id,
                                        endpoint = %candidate.endpoint,
                                        device_id = candidate.device_id.as_deref().unwrap_or("unknown"),
                                        device_name = candidate.device_name.as_deref().unwrap_or("unknown"),
                                        status = %candidate.status,
                                        provisioning_mode = candidate.provisioning_mode,
                                        bind_required = candidate.bind_required,
                                        "embedded accessory discovered"
                                    );
                                    event_bus.publish(ServerEvent::DeviceDiscovered {
                                        channel_id: channel_id.clone(),
                                        endpoint: candidate.endpoint.clone(),
                                        device_id: candidate.device_id.clone(),
                                        device_name: candidate.device_name.clone(),
                                        status: candidate.status.clone(),
                                        provisioning_mode: candidate.provisioning_mode,
                                        bind_required: candidate.bind_required,
                                    });
                                    if candidate.bind_required {
                                        *status.write().await = ConnectionStatus::Disconnected {
                                            since: Utc::now(),
                                            reason: "embedded discovered and awaiting user confirmation".to_string(),
                                        };
                                    }
                                }
                                None => {
                                    tracing::info!(channel_id = %channel_id, "embedded accessory discovery cleared");
                                }
                            }
                            *stored = next_candidate;
                        }
                    }
                }
            }
        });
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
                    *self.audio_client.write().await = Some(configure_audio_client(
                        AudioStreamClient::new(channel.clone()),
                    ));
                    *self.process_client.write().await =
                        Some(ProcessManagerClient::new(channel.clone()));
                    *self.plugin_client.write().await =
                        Some(PluginManagerClient::new(channel.clone()));
                    *self.binding_client.write().await =
                        Some(DeviceBindingClient::new(channel.clone()));
                    *self.settings_client.write().await =
                        Some(EmbeddedSettingsClient::new(channel));
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

    /// Get an EmbeddedSettingsClient, connecting lazily if needed.
    pub async fn settings_client(&self) -> anyhow::Result<EmbeddedSettingsClient<Channel>> {
        if self.settings_client.read().await.is_none() {
            self.connect().await?;
        }
        self.settings_client
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
        Ok(())
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.spawn_discovery_loop();
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.discovery_cancel.cancel();
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
            audio_streaming: true,
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
            AudioFormat::Wav => ProtoAudioEncoding::PcmS16le as i32,
        };

        let audio = normalize_embedded_playback_payload(audio)?;

        let request = PlayAudioRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            source: "tts".into(),
            encoding,
            sample_rate: audio.sample_rate,
            interrupt_current: false,
        };

        // Prime the playback session on the embedded node before sending raw PCM bytes.
        let mut response_stream = client
            .play_audio(self.auth_request(request)?)
            .await?
            .into_inner();

        if let Some(ack) = response_stream.message().await? {
            tracing::trace!(seq = ack.sequence, "play_audio primed on Pi");
        }

        let mut upload_client = self.audio_client().await?;
        let data = audio.data;
        let chunk_count = data.len().div_ceil(AUDIO_GRPC_CHUNK_BYTES);
        tracing::debug!(
            channel_id = %self.config.id,
            bytes = data.len(),
            chunks = chunk_count,
            sample_rate = audio.sample_rate,
            "uploading embedded playback audio in gRPC chunks"
        );
        let upload_stream = async_stream::stream! {
            for (sequence, bytes) in data.chunks(AUDIO_GRPC_CHUNK_BYTES).enumerate() {
                yield AudioChunk {
                opus_data: bytes.to_vec(),
                sequence: sequence as u64,
                timestamp_us: chrono::Utc::now().timestamp_micros() as u64,
                encoding,
                };
            }
        };
        upload_client
            .upload_audio(self.auth_request(upload_stream)?)
            .await?;

        Ok(())
    }

    async fn send_audio_stream(
        &self,
        _target: &ChannelTarget,
        mut audio: AudioPayloadStream,
    ) -> anyhow::Result<()> {
        let Some(first) = audio.next().await else {
            return Ok(());
        };
        let first = normalize_embedded_playback_payload(first?)?;
        let encoding = match first.format {
            AudioFormat::Opus => ProtoAudioEncoding::Opus as i32,
            AudioFormat::Pcm16 | AudioFormat::Wav => ProtoAudioEncoding::PcmS16le as i32,
        };

        let request = PlayAudioRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            source: "tts".into(),
            encoding,
            sample_rate: first.sample_rate,
            interrupt_current: false,
        };

        let mut client = self.audio_client().await?;
        let mut response_stream = client
            .play_audio(self.auth_request(request)?)
            .await?
            .into_inner();

        if let Some(ack) = response_stream.message().await? {
            tracing::trace!(seq = ack.sequence, "streaming play_audio primed on Pi");
        }

        let (tx, rx) = mpsc::channel(AUDIO_STREAM_BACKPRESSURE_CHUNKS);
        let producer = tokio::spawn(async move {
            produce_streaming_audio_chunks(first, audio, tx, encoding).await
        });

        let mut upload_client = self.audio_client().await?;
        let upload_result = upload_client
            .upload_audio(self.auth_request(tokio_stream::wrappers::ReceiverStream::new(rx))?)
            .await;

        let producer_result = producer
            .await
            .map_err(|error| anyhow::anyhow!("audio stream producer join failed: {error}"));
        upload_result?;
        producer_result??;
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
            "restart_process" => {
                let name = action.params["name"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("restart_process requires 'name' param"))?;
                let mut client = self.process_client().await?;
                let resp = client
                    .restart_process(self.auth_request(
                        alia_proto::process::RestartProcessRequest {
                            name: name.to_string(),
                        },
                    )?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "message": resp.message,
                    "pid": resp.pid,
                }))
            }
            "list_processes" => {
                let mut client = self.process_client().await?;
                let resp = client
                    .list_processes(
                        self.auth_request(alia_proto::process::ListProcessesRequest {})?,
                    )
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "processes": resp.processes.into_iter().map(|process| serde_json::json!({
                        "name": process.name,
                        "state": process.state,
                        "pid": process.pid,
                        "uptime_secs": process.uptime_secs,
                        "restart_count": process.restart_count,
                    })).collect::<Vec<_>>(),
                }))
            }
            "list_plugins" => {
                let mut client = self.plugin_client().await?;
                let resp = client
                    .list_plugins(self.auth_request(alia_proto::plugin::ListPluginsRequest {})?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "plugins": resp.plugins.into_iter().map(|plugin| serde_json::json!({
                        "name": plugin.name,
                        "version": plugin.version,
                        "plugin_type": plugin.plugin_type,
                        "state": plugin.state,
                        "description": plugin.description,
                    })).collect::<Vec<_>>(),
                }))
            }
            "install_plugin" => {
                let name = action.params["name"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("install_plugin requires 'name' param"))?;
                let version = action.params["version"].as_str().unwrap_or("");
                let zip_base64 = action.params["zip_base64"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("install_plugin requires 'zip_base64' param"))?;
                let zip_data = base64::engine::general_purpose::STANDARD.decode(zip_base64)?;
                let mut client = self.plugin_client().await?;
                let resp = client
                    .install_plugin(self.auth_request(
                        alia_proto::plugin::InstallPluginRequest {
                            name: name.to_string(),
                            version: version.to_string(),
                            zip_data,
                        },
                    )?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "message": resp.message,
                }))
            }
            "uninstall_plugin" => {
                let name = action.params["name"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("uninstall_plugin requires 'name' param"))?;
                let mut client = self.plugin_client().await?;
                let resp = client
                    .uninstall_plugin(self.auth_request(
                        alia_proto::plugin::UninstallPluginRequest {
                            name: name.to_string(),
                        },
                    )?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "message": resp.message,
                }))
            }
            "start_plugin" => {
                let name = action.params["name"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("start_plugin requires 'name' param"))?;
                let mut client = self.plugin_client().await?;
                let resp = client
                    .start_plugin(self.auth_request(alia_proto::plugin::StartPluginRequest {
                        name: name.to_string(),
                    })?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "message": resp.message,
                }))
            }
            "stop_plugin" => {
                let name = action.params["name"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("stop_plugin requires 'name' param"))?;
                let mut client = self.plugin_client().await?;
                let resp = client
                    .stop_plugin(self.auth_request(alia_proto::plugin::StopPluginRequest {
                        name: name.to_string(),
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
            "discovery_status" => Ok(serde_json::json!({
                "candidate": self.discovered_candidate.read().await.clone(),
            })),
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
                if action.params["require_confirmation"]
                    .as_bool()
                    .unwrap_or(true)
                {
                    let candidate =
                        self.discovered_candidate
                            .read()
                            .await
                            .clone()
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "no discovered embedded accessory is awaiting confirmation"
                                )
                            })?;
                    if !candidate.bind_required {
                        anyhow::bail!("discovered embedded accessory is not available for binding");
                    }
                }
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
            "get_settings" => {
                let mut client = self.settings_client().await?;
                let resp = client
                    .get_settings(self.auth_request(())?)
                    .await?
                    .into_inner();
                let settings = serde_json::from_str::<serde_json::Value>(&resp.settings_json)
                    .unwrap_or_else(|_| serde_json::json!({}));
                Ok(serde_json::json!({
                    "settings": settings,
                    "provisioning_mode": resp.provisioning_mode,
                    "requires_restart": resp.requires_restart,
                    "settings_path": resp.settings_path,
                }))
            }
            "update_settings" => {
                let mut client = self.settings_client().await?;
                let resp = client
                    .update_settings(self.auth_request(
                        alia_proto::settings::UpdateSettingsRequest {
                            settings_json: serde_json::to_string(&action.params)?,
                        },
                    )?)
                    .await?
                    .into_inner();
                let settings = serde_json::from_str::<serde_json::Value>(&resp.settings_json)
                    .unwrap_or_else(|_| serde_json::json!({}));
                Ok(serde_json::json!({
                    "settings": settings,
                    "provisioning_mode": resp.provisioning_mode,
                    "requires_restart": resp.requires_restart,
                    "settings_path": resp.settings_path,
                }))
            }
            "list_direct_responses" => {
                let mut client = self.settings_client().await?;
                let resp = client
                    .list_direct_responses(self.auth_request(())?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "rules": resp.rules.into_iter().map(direct_response_rule_to_json).collect::<Vec<_>>(),
                    "requires_restart": resp.requires_restart,
                }))
            }
            "upsert_direct_response" => {
                let mut client = self.settings_client().await?;
                let rule = direct_response_rule_from_json(&action.params)?;
                let resp = client
                    .upsert_direct_response(self.auth_request(rule)?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "rule": direct_response_rule_to_json(resp),
                    "requires_restart": true,
                }))
            }
            "delete_direct_response" => {
                let id = action.params["id"].as_i64().ok_or_else(|| {
                    anyhow::anyhow!("delete_direct_response requires numeric 'id'")
                })?;
                let mut client = self.settings_client().await?;
                let resp =
                    client
                        .delete_direct_response(self.auth_request(
                            alia_proto::settings::DeleteDirectResponseRequest { id },
                        )?)
                        .await?
                        .into_inner();
                Ok(serde_json::json!({
                    "success": resp.success,
                    "requires_restart": resp.requires_restart,
                }))
            }
            _ => Err(anyhow::anyhow!(
                "action '{}' not supported by embedded channel",
                action.name
            )),
        }
    }
}

async fn produce_streaming_audio_chunks(
    first: AudioPayload,
    mut rest: AudioPayloadStream,
    tx: mpsc::Sender<AudioChunk>,
    encoding: i32,
) -> anyhow::Result<()> {
    let mut chunker = StreamingAudioChunker::new(tx, encoding, AUDIO_STREAM_GRPC_CHUNK_BYTES);
    let mut wav_decoder = StreamingWavPcmDecoder::default();

    process_stream_payload(first, &mut chunker, &mut wav_decoder).await?;
    while let Some(payload) = rest.next().await {
        process_stream_payload(
            normalize_embedded_playback_payload(payload?)?,
            &mut chunker,
            &mut wav_decoder,
        )
        .await?;
    }

    chunker.flush().await
}

async fn process_stream_payload(
    payload: AudioPayload,
    chunker: &mut StreamingAudioChunker,
    wav_decoder: &mut StreamingWavPcmDecoder,
) -> anyhow::Result<()> {
    match payload.format {
        AudioFormat::Wav => {
            let pcm = wav_decoder.push(&payload.data)?;
            if !pcm.is_empty() {
                chunker.push(&pcm).await?;
            }
        }
        AudioFormat::Pcm16 | AudioFormat::Opus => {
            chunker.push(&payload.data).await?;
        }
    }
    Ok(())
}

struct StreamingAudioChunker {
    tx: mpsc::Sender<AudioChunk>,
    encoding: i32,
    max_bytes: usize,
    sequence: u64,
    pending: Vec<u8>,
}

impl StreamingAudioChunker {
    fn new(tx: mpsc::Sender<AudioChunk>, encoding: i32, max_bytes: usize) -> Self {
        Self {
            tx,
            encoding,
            max_bytes,
            sequence: 0,
            pending: Vec::with_capacity(max_bytes),
        }
    }

    async fn push(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.pending.extend_from_slice(bytes);
        while self.pending.len() >= self.max_bytes {
            let tail = self.pending.split_off(self.max_bytes);
            let chunk = std::mem::replace(&mut self.pending, tail);
            self.send(chunk).await?;
        }
        Ok(())
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        if !self.pending.is_empty() {
            let chunk = std::mem::take(&mut self.pending);
            self.send(chunk).await?;
        }
        Ok(())
    }

    async fn send(&mut self, bytes: Vec<u8>) -> anyhow::Result<()> {
        let chunk = AudioChunk {
            opus_data: bytes,
            sequence: self.sequence,
            timestamp_us: chrono::Utc::now().timestamp_micros() as u64,
            encoding: self.encoding,
        };
        self.sequence += 1;
        self.tx
            .send(chunk)
            .await
            .map_err(|_| anyhow::anyhow!("embedded audio upload stream closed"))
    }
}

#[derive(Default)]
struct StreamingWavPcmDecoder {
    header: Vec<u8>,
    data_started: bool,
    pending_odd: Option<u8>,
}

impl StreamingWavPcmDecoder {
    fn push(&mut self, bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
        if self.data_started && bytes.starts_with(b"RIFF") {
            *self = Self::default();
        }

        let mut pcm = if self.data_started {
            bytes.to_vec()
        } else {
            self.header.extend_from_slice(bytes);
            let Some(data_start) = wav_data_start(&self.header)? else {
                return Ok(Vec::new());
            };
            let pcm = self.header[data_start..].to_vec();
            self.header.clear();
            self.data_started = true;
            pcm
        };

        if let Some(odd) = self.pending_odd.take() {
            let mut with_odd = Vec::with_capacity(pcm.len() + 1);
            with_odd.push(odd);
            with_odd.extend_from_slice(&pcm);
            pcm = with_odd;
        }
        if pcm.len() % 2 != 0 {
            self.pending_odd = pcm.pop();
        }

        Ok(pcm)
    }
}

fn wav_data_start(buffer: &[u8]) -> anyhow::Result<Option<usize>> {
    if buffer.len() < 12 {
        return Ok(None);
    }
    if &buffer[0..4] != b"RIFF" || &buffer[8..12] != b"WAVE" {
        anyhow::bail!("streaming WAV payload missing RIFF/WAVE header");
    }

    let mut pos = 12usize;
    while pos + 8 <= buffer.len() {
        let chunk_id = &buffer[pos..pos + 4];
        let size = u32::from_le_bytes(buffer[pos + 4..pos + 8].try_into()?) as usize;
        let data_start = pos + 8;
        if chunk_id == b"data" {
            return Ok(Some(data_start));
        }
        let padded = size + (size % 2);
        let next = data_start.saturating_add(padded);
        if next > buffer.len() {
            return Ok(None);
        }
        pos = next;
    }

    if buffer.len() > 128 * 1024 {
        anyhow::bail!("streaming WAV header exceeded 128 KiB without data chunk");
    }
    Ok(None)
}

fn direct_response_rule_from_json(
    value: &serde_json::Value,
) -> anyhow::Result<alia_proto::settings::DirectResponseRule> {
    let keywords = value
        .get("keywords")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("direct response requires 'keywords' array"))?
        .iter()
        .map(|keyword| {
            keyword
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("direct response keywords must be strings"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let audio_path = value
        .get("audio_path")
        .or_else(|| value.get("audio"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let audio_variants = direct_response_audio_variants_from_json(value)?;

    if audio_path.trim().is_empty() && audio_variants.is_empty() {
        anyhow::bail!("direct response requires 'audio_path' or 'audio_variants'");
    }

    Ok(alia_proto::settings::DirectResponseRule {
        id: value
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        slug: value
            .get("slug")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        keywords,
        audio_path,
        action: value
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        enabled: value
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
        audio_variants,
    })
}

fn direct_response_rule_to_json(
    rule: alia_proto::settings::DirectResponseRule,
) -> serde_json::Value {
    let audio_variants = rule
        .audio_variants
        .into_iter()
        .map(|variant| {
            serde_json::json!({
                "audio_path": variant.audio_path,
                "weight": variant.weight,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "id": rule.id,
        "slug": rule.slug,
        "keywords": rule.keywords,
        "audio_path": rule.audio_path,
        "audio_variants": audio_variants,
        "action": if rule.action.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(rule.action) },
        "enabled": rule.enabled,
    })
}

fn direct_response_audio_variants_from_json(
    value: &serde_json::Value,
) -> anyhow::Result<Vec<alia_proto::settings::DirectResponseAudioVariant>> {
    let Some(variants) = value
        .get("audio_variants")
        .or_else(|| value.get("audioVariants"))
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(Vec::new());
    };

    variants
        .iter()
        .map(|variant| {
            let audio_path = variant
                .get("audio_path")
                .or_else(|| variant.get("audio"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let audio_data = variant
                .get("audio_base64")
                .or_else(|| variant.get("audioDataBase64"))
                .and_then(serde_json::Value::as_str)
                .map(|payload| base64::engine::general_purpose::STANDARD.decode(payload))
                .transpose()?
                .unwrap_or_default();
            let weight = variant
                .get("weight")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(1.0) as f32;

            Ok(alia_proto::settings::DirectResponseAudioVariant {
                audio_path,
                weight,
                audio_data,
            })
        })
        .collect()
}

fn configure_audio_client(client: AudioStreamClient<Channel>) -> AudioStreamClient<Channel> {
    client
        .max_encoding_message_size(GRPC_MESSAGE_LIMIT_BYTES)
        .max_decoding_message_size(GRPC_MESSAGE_LIMIT_BYTES)
}

fn normalize_embedded_playback_payload(audio: AudioPayload) -> anyhow::Result<AudioPayload> {
    match audio.format {
        AudioFormat::Wav => {
            if let Some((pcm, sample_rate)) = pcm16_from_wav(&audio.data)? {
                Ok(AudioPayload {
                    data: pcm16_to_le_bytes(&pcm),
                    sample_rate,
                    channels: 1,
                    format: AudioFormat::Pcm16,
                })
            } else {
                Ok(AudioPayload {
                    format: AudioFormat::Pcm16,
                    ..audio
                })
            }
        }
        AudioFormat::Pcm16 if audio.channels > 1 => {
            let pcm = pcm16le_bytes_to_samples(&audio.data)?;
            let mono = downmix_pcm16(&pcm, audio.channels as usize);
            Ok(AudioPayload {
                data: pcm16_to_le_bytes(&mono),
                channels: 1,
                ..audio
            })
        }
        _ => Ok(audio),
    }
}

fn pcm16_from_wav(bytes: &[u8]) -> anyhow::Result<Option<(Vec<i16>, u32)>> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Ok(None);
    }

    let mut offset = 12;
    let mut sample_rate = None;
    let mut channels = None;
    let mut bits_per_sample = None;
    let mut audio_format = None;
    let mut pcm_bytes = None;

    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?) as usize;
        offset += 8;
        if offset + size > bytes.len() {
            anyhow::bail!("malformed WAV chunk");
        }

        match id {
            b"fmt " => {
                if size < 16 {
                    anyhow::bail!("invalid WAV fmt chunk");
                }
                audio_format = Some(u16::from_le_bytes(bytes[offset..offset + 2].try_into()?));
                channels = Some(u16::from_le_bytes(
                    bytes[offset + 2..offset + 4].try_into()?,
                ));
                sample_rate = Some(u32::from_le_bytes(
                    bytes[offset + 4..offset + 8].try_into()?,
                ));
                bits_per_sample = Some(u16::from_le_bytes(
                    bytes[offset + 14..offset + 16].try_into()?,
                ));
            }
            b"data" => pcm_bytes = Some(&bytes[offset..offset + size]),
            _ => {}
        }

        offset += size + (size % 2);
    }

    if audio_format != Some(1) || bits_per_sample != Some(16) {
        anyhow::bail!("expected 16-bit PCM WAV");
    }

    let channels = channels.unwrap_or(1).max(1) as usize;
    let sample_rate = sample_rate.unwrap_or(24_000);
    let samples: Vec<i16> = pcm_bytes
        .ok_or_else(|| anyhow::anyhow!("missing WAV data chunk"))?
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    if channels == 1 {
        return Ok(Some((samples, sample_rate)));
    }

    Ok(Some((
        samples
            .chunks_exact(channels)
            .map(|frame| frame[0])
            .collect(),
        sample_rate,
    )))
}

fn pcm16_to_le_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}

fn pcm16le_bytes_to_samples(bytes: &[u8]) -> anyhow::Result<Vec<i16>> {
    if bytes.len() % 2 != 0 {
        anyhow::bail!("PCM16LE byte payload must have even length");
    }

    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect())
}

fn downmix_pcm16(samples: &[i16], channels: usize) -> Vec<i16> {
    if channels <= 1 {
        return samples.to_vec();
    }

    samples
        .chunks_exact(channels)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|sample| i32::from(*sample)).sum();
            (sum / frame.len() as i32) as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_stereo_pcm16_payload_to_mono() {
        let payload = AudioPayload {
            data: pcm16_to_le_bytes(&[100, 300, -200, -400]),
            sample_rate: 24_000,
            channels: 2,
            format: AudioFormat::Pcm16,
        };

        let normalized = normalize_embedded_playback_payload(payload).unwrap();

        assert_eq!(normalized.sample_rate, 24_000);
        assert_eq!(normalized.channels, 1);
        assert_eq!(normalized.format, AudioFormat::Pcm16);
        assert_eq!(
            pcm16le_bytes_to_samples(&normalized.data).unwrap(),
            vec![200, -300]
        );
    }
}
