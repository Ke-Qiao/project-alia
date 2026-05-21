use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use alia_transport::{
    DEFAULT_AUTH_MAX_SKEW, EMBEDDED_INGRESS_SCOPE, bind_payload, current_unix_timestamp,
    discover_service, discover_service_endpoint, embedded_device_auth_subject,
    handoff_completion_payload, handoff_intent_payload, sign_payload, verify_auth_token,
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
use alia_proto::audio::{
    AudioChunk, AudioEncoding as ProtoAudioEncoding, PlayAudioRequest, PlayDirectResponseRequest,
    StopPlaybackRequest,
};
use alia_proto::binding::device_binding_client::DeviceBindingClient;
use alia_proto::expression::embedded_expression_client::EmbeddedExpressionClient;
use alia_proto::expression::{
    CancelExpressionRequest, ExecuteExpressionRequest, ExpressionCapabilitiesRequest,
};
use alia_proto::plugin::plugin_manager_client::PluginManagerClient;
use alia_proto::sensor::embedded_sensor_client::EmbeddedSensorClient;
use alia_proto::settings::embedded_settings_client::EmbeddedSettingsClient;

use crate::auth::load_server_keypair;
use crate::event::{EventBus, ServerEvent};
use crate::module::ModuleLifecycle;
use crate::module::lifecycle::{ModuleContext, ModuleHealth};
use crate::providers::sensor::SensorRegistry;

use super::audio::{
    AUDIO_GRPC_CHUNK_BYTES, AUDIO_STREAM_BACKPRESSURE_CHUNKS, configure_audio_client,
    normalize_embedded_playback_payload, produce_streaming_audio_chunks,
};
use super::auth::auth_request_with_signing_key;
use super::direct_response::{direct_response_rule_from_json, direct_response_rule_to_json};
use super::sensor::{
    EmbeddedSensorReadProvider, embedded_sensor_command_response_to_json,
    embedded_sensor_definition_to_json, embedded_sensor_reading_to_json, embedded_sensor_remote_id,
    embedded_sensor_source, ingest_embedded_sensor_reading, mirror_embedded_sensor_definitions,
};
use super::settings::{required_action_str, set_volume_request_from_json, volume_settings_to_json};
use crate::channels::adapter::{
    AudioFormat, AudioPayload, AudioPayloadStream, ChannelAction, ChannelAdapter,
    ChannelCapabilities, ChannelInterruptConfig, ChannelTarget,
};

const SENSOR_SYNC_INTERVAL_SEC: u64 = 30;

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
    pub voice_turn_interrupt_enabled: bool,
    pub cancel_streaming_tts_on_interrupt: bool,
    pub discard_interrupted_turn_history: bool,
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
#[derive(Clone)]
pub struct EmbeddedChannel {
    config: EmbeddedChannelConfig,
    event_bus: EventBus,
    status: Arc<RwLock<ConnectionStatus>>,
    audio_client: Arc<RwLock<Option<AudioStreamClient<Channel>>>>,
    plugin_client: Arc<RwLock<Option<PluginManagerClient<Channel>>>>,
    binding_client: Arc<RwLock<Option<DeviceBindingClient<Channel>>>>,
    settings_client: Arc<RwLock<Option<EmbeddedSettingsClient<Channel>>>>,
    sensor_client: Arc<RwLock<Option<EmbeddedSensorClient<Channel>>>>,
    expression_client: Arc<RwLock<Option<EmbeddedExpressionClient<Channel>>>>,
    sensor_registry: Arc<RwLock<Option<Arc<SensorRegistry>>>>,
    discovered_candidate: Arc<RwLock<Option<DiscoveryCandidate>>>,
    discovery_cancel: CancellationToken,
    signing_key: Option<SigningKey>,
    server_public_key: Vec<u8>,
    device_id: Arc<RwLock<Option<String>>>,
    device_public_key: Arc<RwLock<Option<Vec<u8>>>>,
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
            plugin_client: Arc::new(RwLock::new(None)),
            binding_client: Arc::new(RwLock::new(None)),
            settings_client: Arc::new(RwLock::new(None)),
            sensor_client: Arc::new(RwLock::new(None)),
            expression_client: Arc::new(RwLock::new(None)),
            sensor_registry: Arc::new(RwLock::new(None)),
            discovered_candidate: Arc::new(RwLock::new(None)),
            discovery_cancel: CancellationToken::new(),
            signing_key: None,
            server_public_key: Vec::new(),
            device_id: Arc::new(RwLock::new(None)),
            device_public_key: Arc::new(RwLock::new(None)),
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

    fn spawn_sensor_sync_loop(&self) {
        let cancel = self.discovery_cancel.child_token();
        let sensor_client = self.sensor_client.clone();
        let sensor_registry = self.sensor_registry.clone();
        let channel_id = self.config.id.clone();
        let signing_key = self.signing_key.clone();
        let server_id = self.default_server_id.clone();

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(SENSOR_SYNC_INTERVAL_SEC));
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        let Some(registry) = sensor_registry.read().await.clone() else {
                            continue;
                        };
                        let Some(mut client) = sensor_client.read().await.clone() else {
                            continue;
                        };
                        let Some(signing_key) = signing_key.as_ref() else {
                            tracing::warn!(
                                channel_id = %channel_id,
                                "embedded sensor sync skipped because server signing key is unavailable"
                            );
                            continue;
                        };

                        let request = match auth_request_with_signing_key(signing_key, &server_id, ()) {
                            Ok(request) => request,
                            Err(error) => {
                                tracing::warn!(
                                    channel_id = %channel_id,
                                    error = %error,
                                    "failed to build embedded sensor sync auth request"
                                );
                                continue;
                            }
                        };

                        match client.list_sensors(request).await {
                            Ok(response) => {
                                if let Err(error) = mirror_embedded_sensor_definitions(
                                    &registry,
                                    &channel_id,
                                    response.into_inner().sensors,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        channel_id = %channel_id,
                                        error = %error,
                                        "failed to mirror embedded sensors during periodic sync"
                                    );
                                }
                            }
                            Err(error) => {
                                tracing::debug!(
                                    channel_id = %channel_id,
                                    error = %error,
                                    "embedded sensor periodic sync failed"
                                );
                            }
                        }
                    }
                }
            }
        });
    }

    fn spawn_connection_loop(&self) {
        let channel = self.clone();
        let cancel = self.discovery_cancel.child_token();

        tokio::spawn(async move {
            let base_delay = Duration::from_secs(channel.config.reconnect_interval_sec.max(1));
            let max_delay = Duration::from_secs(channel.config.reconnect_max_backoff_sec.max(1));
            let mut delay = Duration::ZERO;

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(delay) => {
                        let connected = matches!(
                            &*channel.status.read().await,
                            ConnectionStatus::Connected
                        ) && channel.sensor_client.read().await.is_some();

                        if connected {
                            delay = base_delay;
                            continue;
                        }

                        if let Err(error) = channel.connect().await {
                            tracing::warn!(
                                channel_id = %channel.config.id,
                                error = %error,
                                "embedded connection attempt failed"
                            );
                        }

                        let connected = matches!(
                            &*channel.status.read().await,
                            ConnectionStatus::Connected
                        ) && channel.sensor_client.read().await.is_some();

                        delay = if connected {
                            base_delay
                        } else if delay.is_zero() {
                            base_delay
                        } else {
                            std::cmp::min(delay.saturating_mul(2), max_delay)
                        };
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
                    *self.plugin_client.write().await =
                        Some(PluginManagerClient::new(channel.clone()));
                    let binding_client = DeviceBindingClient::new(channel.clone());
                    *self.binding_client.write().await = Some(binding_client.clone());
                    *self.settings_client.write().await =
                        Some(EmbeddedSettingsClient::new(channel.clone()));
                    *self.expression_client.write().await =
                        Some(EmbeddedExpressionClient::new(channel.clone()));
                    let sensor_client = EmbeddedSensorClient::new(channel);
                    *self.sensor_client.write().await = Some(sensor_client.clone());
                    *self.status.write().await = ConnectionStatus::Connected;
                    if let Err(error) = self
                        .refresh_bound_device_from_status_with_client(binding_client)
                        .await
                    {
                        tracing::warn!(
                            channel_id = %self.config.id,
                            error = %error,
                            "failed to refresh embedded binding identity"
                        );
                    }
                    if let Err(error) = self.sync_embedded_sensors_with_client(sensor_client).await
                    {
                        tracing::warn!(
                            channel_id = %self.config.id,
                            error = %error,
                            "failed to sync embedded sensor definitions"
                        );
                    }
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
                        if let Err(error) = self.mark_embedded_sensors_offline().await {
                            tracing::warn!(
                                channel_id = %self.config.id,
                                error = %error,
                                "failed to mark embedded sensors offline after disconnect"
                            );
                        }
                        return Ok(());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500 * attempts as u64))
                        .await;
                }
            }
        }
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

    /// Get an EmbeddedSensorClient, connecting lazily if needed.
    pub async fn sensor_client(&self) -> anyhow::Result<EmbeddedSensorClient<Channel>> {
        if self.sensor_client.read().await.is_none() {
            self.connect().await?;
        }
        self.sensor_client
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("embedded not connected"))
    }

    pub async fn expression_client(&self) -> anyhow::Result<EmbeddedExpressionClient<Channel>> {
        if self.expression_client.read().await.is_none() {
            self.connect().await?;
        }
        self.expression_client
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("embedded not connected"))
    }

    async fn sync_embedded_sensors(&self) -> anyhow::Result<usize> {
        if self.sensor_client.read().await.is_none() {
            self.connect().await?;
        }
        let client = self
            .sensor_client
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("embedded not connected"))?;
        self.sync_embedded_sensors_with_client(client).await
    }

    async fn sync_embedded_sensors_with_client(
        &self,
        mut client: EmbeddedSensorClient<Channel>,
    ) -> anyhow::Result<usize> {
        let Some(registry) = self.sensor_registry.read().await.clone() else {
            return Ok(0);
        };
        let sensors = client
            .list_sensors(self.auth_request(())?)
            .await?
            .into_inner()
            .sensors;
        mirror_embedded_sensor_definitions(&registry, &self.config.id, sensors).await
    }

    async fn mark_embedded_sensors_offline(&self) -> anyhow::Result<usize> {
        let Some(registry) = self.sensor_registry.read().await.clone() else {
            return Ok(0);
        };
        registry
            .mark_source_sensors_offline_except(
                &embedded_sensor_source(&self.config.id),
                &HashSet::new(),
            )
            .await
    }

    fn auth_request<T>(&self, message: T) -> anyhow::Result<Request<T>> {
        let signing_key = self
            .signing_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("server signing key not initialized"))?;
        auth_request_with_signing_key(signing_key, &self.default_server_id, message)
    }

    async fn verify_audio_ingress_auth(&self, token: &str) -> anyhow::Result<()> {
        let device_public_key = self
            .device_public_key
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("embedded device public key is not available"))?;
        let device_id = self.device_id.read().await.clone().unwrap_or_default();
        let subject = embedded_device_auth_subject(&device_id, &device_public_key);
        verify_auth_token(
            token,
            &device_public_key,
            &subject,
            EMBEDDED_INGRESS_SCOPE,
            DEFAULT_AUTH_MAX_SKEW,
        )?;
        Ok(())
    }

    async fn remember_bound_device(
        &self,
        device_id: String,
        device_public_key: Vec<u8>,
    ) -> anyhow::Result<()> {
        if device_public_key.is_empty() {
            anyhow::bail!("embedded bind response did not include device public key");
        }
        *self.device_id.write().await = Some(device_id);
        *self.device_public_key.write().await = Some(device_public_key);
        Ok(())
    }

    async fn refresh_bound_device_from_status_with_client(
        &self,
        mut client: DeviceBindingClient<Channel>,
    ) -> anyhow::Result<()> {
        let status = client
            .get_binding_status(self.auth_request(())?)
            .await?
            .into_inner();
        if status.is_bound && !status.device_public_key.is_empty() {
            self.remember_bound_device(status.device_id, status.device_public_key)
                .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl ModuleLifecycle for EmbeddedChannel {
    fn name(&self) -> &'static str {
        "embedded-channel"
    }

    async fn init(&mut self, _ctx: &ModuleContext) -> anyhow::Result<()> {
        *self.sensor_registry.write().await = Some(_ctx.sensors.clone());
        let keypair = load_server_keypair(&_ctx.config.auth.server_keypair_path)?;
        self.server_public_key = keypair.public_key_bytes().to_vec();
        self.signing_key = Some(keypair.signing_key().clone());
        self.default_server_id = _ctx.config.server.id.clone();
        self.default_server_name = format!("alia-server@{}", _ctx.config.server.host);
        self.default_server_endpoint = default_server_endpoint_for_embedded(
            &_ctx.config.server.host,
            _ctx.config.server.port,
            _ctx.config.server.public_endpoint.as_deref(),
        );
        let signing_key = self
            .signing_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("server signing key not initialized"))?
            .clone();
        _ctx.sensors
            .register_read_provider(Arc::new(EmbeddedSensorReadProvider {
                channel_id: self.config.id.clone(),
                source: embedded_sensor_source(&self.config.id),
                endpoint: self.config.endpoint.clone(),
                discovery_service_name: self.config.discovery_service_name.clone(),
                discovery_timeout_sec: self.config.discovery_timeout_sec,
                sensor_client: self.sensor_client.clone(),
                signing_key,
                server_id: self.default_server_id.clone(),
            }))
            .await?;
        Ok(())
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.spawn_discovery_loop();
        self.spawn_connection_loop();
        self.spawn_sensor_sync_loop();
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.discovery_cancel.cancel();
        if let Err(error) = self.mark_embedded_sensors_offline().await {
            tracing::warn!(
                channel_id = %self.config.id,
                error = %error,
                "failed to mark embedded sensors offline during shutdown"
            );
        }
        if let Some(registry) = self.sensor_registry.read().await.clone() {
            registry
                .unregister_read_provider(&embedded_sensor_source(&self.config.id))
                .await;
        }
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
            conversation_control: true,
        }
    }

    fn interrupt_config(&self) -> ChannelInterruptConfig {
        ChannelInterruptConfig {
            enabled: self.config.voice_turn_interrupt_enabled,
            cancel_streaming_tts_on_interrupt: self.config.cancel_streaming_tts_on_interrupt,
            discard_interrupted_turn_history: self.config.discard_interrupted_turn_history,
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

    async fn stop_current_playback(
        &self,
        _target: &ChannelTarget,
        reason: &str,
    ) -> anyhow::Result<()> {
        let mut client = self.audio_client().await?;
        client
            .stop_playback(self.auth_request(StopPlaybackRequest {
                request_id: uuid::Uuid::new_v4().to_string(),
                reason: reason.to_string(),
            })?)
            .await?;
        Ok(())
    }

    async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
        // Route actions to the appropriate gRPC service on the Pi
        match action.name.as_str() {
            "verify_audio_ingress_auth" => {
                let token = required_action_str(&action.params, "token")?;
                self.verify_audio_ingress_auth(token).await?;
                Ok(serde_json::json!({ "ok": true }))
            }
            "play_direct_response" => {
                let direct_action = required_action_str(&action.params, "action")?;
                let interrupt_current = action
                    .params
                    .get("interrupt_current")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true);
                let reason = action
                    .params
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("server_requested_direct_response");
                let mut client = self.audio_client().await?;
                let resp = client
                    .play_direct_response(self.auth_request(PlayDirectResponseRequest {
                        request_id: uuid::Uuid::new_v4().to_string(),
                        action: direct_action.to_string(),
                        interrupt_current,
                        reason: reason.to_string(),
                    })?)
                    .await?
                    .into_inner();
                Ok(serde_json::json!({
                    "accepted": resp.accepted,
                    "played": resp.played,
                    "message": resp.message,
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
                        "capabilities": plugin.capabilities,
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
                    "device_id": resp.device_id,
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
                if server_endpoint.trim().is_empty() {
                    anyhow::bail!(
                        "bind_device requires server_endpoint when server.host is a wildcard address; configure server.public_endpoint or pass a LAN-reachable endpoint"
                    );
                }
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
                if resp.success {
                    self.remember_bound_device(
                        resp.device_id.clone(),
                        resp.device_public_key.clone(),
                    )
                    .await?;
                }
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
                if resp.success {
                    *self.device_id.write().await = None;
                    *self.device_public_key.write().await = None;
                }
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
                if resp.success {
                    self.remember_bound_device(
                        resp.device_id.clone(),
                        resp.device_public_key.clone(),
                    )
                    .await?;
                }
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
            "get_volume" => {
                let mut client = self.settings_client().await?;
                let resp = client
                    .get_volume(self.auth_request(())?)
                    .await?
                    .into_inner();
                Ok(volume_settings_to_json(resp))
            }
            "set_volume" => {
                let mut client = self.settings_client().await?;
                let request = set_volume_request_from_json(&action.params)?;
                let resp = client
                    .set_volume(self.auth_request(request)?)
                    .await?
                    .into_inner();
                Ok(volume_settings_to_json(resp))
            }
            "expression_capabilities" => {
                let mut client = self.expression_client().await?;
                let resp = client
                    .get_capabilities(self.auth_request(ExpressionCapabilitiesRequest {})?)
                    .await?
                    .into_inner();
                Ok(serde_json::from_str::<serde_json::Value>(
                    &resp.capabilities_json,
                )?)
            }
            "execute_expression" => {
                let mut client = self.expression_client().await?;
                let resp = client
                    .execute_expression(self.auth_request(ExecuteExpressionRequest {
                        plan_json: serde_json::to_string(&action.params)?,
                    })?)
                    .await?
                    .into_inner();
                Ok(serde_json::from_str::<serde_json::Value>(
                    &resp.result_json,
                )?)
            }
            "cancel_expression" => {
                let mut client = self.expression_client().await?;
                let reason = action
                    .params
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("cancelled by server")
                    .to_string();
                let resp = client
                    .cancel_expression(self.auth_request(CancelExpressionRequest { reason })?)
                    .await?
                    .into_inner();
                Ok(serde_json::from_str::<serde_json::Value>(
                    &resp.result_json,
                )?)
            }
            "end_continuous_dialogue" => {
                let reason = action
                    .params
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|reason| !reason.is_empty())
                    .unwrap_or("server requested continuous dialogue end");
                let control_reason = format!("end_continuous_dialogue:{reason}");
                self.stop_current_playback(
                    &ChannelTarget {
                        user_id: String::new(),
                    },
                    &control_reason,
                )
                .await?;
                Ok(serde_json::json!({
                    "ok": true,
                    "ended": true,
                    "channel_id": self.config.id.clone(),
                    "reason": reason,
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
            "list_embedded_sensors" | "list_sensors" => {
                let mut client = self.sensor_client().await?;
                let resp = client
                    .list_sensors(self.auth_request(())?)
                    .await?
                    .into_inner();
                let sensors = resp.sensors;
                if let Some(registry) = self.sensor_registry.read().await.clone()
                    && let Err(error) = mirror_embedded_sensor_definitions(
                        &registry,
                        &self.config.id,
                        sensors.clone(),
                    )
                    .await
                {
                    tracing::warn!(
                        channel_id = %self.config.id,
                        error = %error,
                        "failed to mirror listed embedded sensors into server registry"
                    );
                }
                Ok(serde_json::json!({
                    "sensors": sensors.into_iter().map(|definition| embedded_sensor_definition_to_json(&self.config.id, definition)).collect::<Vec<_>>(),
                }))
            }
            "read_embedded_sensor" | "read_sensor" => {
                let sensor_id = required_action_str(&action.params, "sensor_id")?;
                let remote_sensor_id = embedded_sensor_remote_id(&self.config.id, sensor_id);
                let mut client = self.sensor_client().await?;
                let resp = client
                    .read_sensor(self.auth_request(alia_proto::sensor::SensorRef {
                        sensor_id: remote_sensor_id,
                    })?)
                    .await?
                    .into_inner();
                if resp.available
                    && let Some(registry) = self.sensor_registry.read().await.clone()
                    && let Err(error) =
                        ingest_embedded_sensor_reading(&registry, &self.config.id, &resp).await
                {
                    tracing::warn!(
                        channel_id = %self.config.id,
                        sensor_id = %sensor_id,
                        error = %error,
                        "failed to ingest embedded sensor reading into server registry"
                    );
                }
                Ok(embedded_sensor_reading_to_json(&self.config.id, resp))
            }
            "invoke_embedded_sensor" | "invoke_sensor" => {
                let sensor_id = required_action_str(&action.params, "sensor_id")?;
                let command = required_action_str(&action.params, "command")?;
                let remote_sensor_id = embedded_sensor_remote_id(&self.config.id, sensor_id);
                let args = action
                    .params
                    .get("args")
                    .cloned()
                    .or_else(|| action.params.get("arguments").cloned())
                    .unwrap_or_else(|| serde_json::json!({}));
                let mut client = self.sensor_client().await?;
                let resp = client
                    .invoke_sensor(self.auth_request(alia_proto::sensor::SensorCommandRequest {
                        sensor_id: remote_sensor_id,
                        command: command.to_string(),
                        args_json: serde_json::to_string(&args)?,
                    })?)
                    .await?
                    .into_inner();
                Ok(embedded_sensor_command_response_to_json(resp))
            }
            _ => Err(anyhow::anyhow!(
                "action '{}' not supported by embedded channel",
                action.name
            )),
        }
    }
}

fn default_server_endpoint_for_embedded(
    host: &str,
    port: u16,
    public_endpoint: Option<&str>,
) -> String {
    if let Some(endpoint) = public_endpoint
        && !endpoint.trim().is_empty()
    {
        return endpoint.trim().to_string();
    }

    if is_unspecified_host(host) {
        tracing::warn!(
            host,
            "server.host is a wildcard address; embedded binding requires server.public_endpoint or explicit server_endpoint"
        );
        String::new()
    } else {
        format!("http://{}:{}", host, port)
    }
}

fn is_unspecified_host(host: &str) -> bool {
    let trimmed = host.trim().trim_matches(['[', ']']);
    trimmed
        .parse::<IpAddr>()
        .map(|addr| addr.is_unspecified())
        .unwrap_or(false)
}
