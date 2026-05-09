use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tonic::transport::Channel;

use alia_proto::binding::device_binding_client::DeviceBindingClient;
use alia_proto::plugin::plugin_manager_client::PluginManagerClient;
use alia_proto::process::process_manager_client::ProcessManagerClient;

use crate::event::EventBus;
use crate::module::lifecycle::{ModuleContext, ModuleHealth};
use crate::module::ModuleLifecycle;

use super::adapter::{
    AudioPayload, ChannelAction, ChannelAdapter, ChannelCapabilities, ChannelTarget,
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

    async fn send_message(&self, _target: &ChannelTarget, _msg: &str) -> anyhow::Result<()> {
        // For the embedded channel, text messages are converted to audio via TTS
        // by the AI pipeline (voice_output = true), then sent via send_audio().
        Ok(())
    }

    async fn send_audio(&self, _target: &ChannelTarget, _audio: AudioPayload) -> anyhow::Result<()> {
        // TODO: use gRPC PlayAudio to stream audio to Pi
        Ok(())
    }

    async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
        // Forward action to Pi via gRPC
        Err(anyhow::anyhow!(
            "action '{}' not yet implemented for embedded channel",
            action.name
        ))
    }
}
