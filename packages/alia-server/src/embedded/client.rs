use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tonic::transport::Channel;

use alia_proto::binding::device_binding_client::DeviceBindingClient;
use alia_proto::plugin::plugin_manager_client::PluginManagerClient;
use alia_proto::process::process_manager_client::ProcessManagerClient;

use crate::event::EventBus;

#[derive(Debug, Clone)]
pub enum ConnectionStatus {
    Connected,
    Disconnected { since: DateTime<Utc>, reason: String },
    Reconnecting { attempt: u32 },
}

/// gRPC client wrapper for communicating with alia-embedded
#[derive(Clone)]
pub struct EmbeddedClient {
    endpoint: String,
    status: Arc<RwLock<ConnectionStatus>>,
    event_bus: EventBus,
    process_client: Arc<RwLock<Option<ProcessManagerClient<Channel>>>>,
    plugin_client: Arc<RwLock<Option<PluginManagerClient<Channel>>>>,
    binding_client: Arc<RwLock<Option<DeviceBindingClient<Channel>>>>,
}

impl EmbeddedClient {
    pub fn new(endpoint: &str, event_bus: EventBus) -> Self {
        let status = if endpoint.is_empty() {
            ConnectionStatus::Disconnected {
                since: Utc::now(),
                reason: "no endpoint configured".into(),
            }
        } else {
            ConnectionStatus::Reconnecting { attempt: 0 }
        };

        Self {
            endpoint: endpoint.to_string(),
            status: Arc::new(RwLock::new(status)),
            event_bus,
            process_client: Arc::new(RwLock::new(None)),
            plugin_client: Arc::new(RwLock::new(None)),
            binding_client: Arc::new(RwLock::new(None)),
        }
    }

    /// Establish gRPC connection to alia-embedded.
    /// Safe to call multiple times — reconnects if not connected.
    pub async fn connect(&self) -> anyhow::Result<()> {
        if self.endpoint.is_empty() {
            return Ok(());
        }

        let mut attempts = 0u32;
        loop {
            attempts += 1;
            *self.status.write().await = ConnectionStatus::Reconnecting { attempt: attempts };

            match Channel::from_shared(self.endpoint.clone())
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
                    tracing::info!(endpoint = %self.endpoint, "connected to alia-embedded");
                    return Ok(());
                }
                Err(e) => {
                    if attempts >= 3 {
                        *self.status.write().await = ConnectionStatus::Disconnected {
                            since: Utc::now(),
                            reason: format!("failed after {attempts} attempts: {e}"),
                        };
                        tracing::warn!(
                            endpoint = %self.endpoint,
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
    pub async fn process_client(
        &self,
    ) -> anyhow::Result<ProcessManagerClient<Channel>> {
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
    pub async fn plugin_client(
        &self,
    ) -> anyhow::Result<PluginManagerClient<Channel>> {
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
    pub async fn binding_client(
        &self,
    ) -> anyhow::Result<DeviceBindingClient<Channel>> {
        if self.binding_client.read().await.is_none() {
            self.connect().await?;
        }
        self.binding_client
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("embedded not connected"))
    }

    pub async fn connection_status(&self) -> ConnectionStatus {
        self.status.read().await.clone()
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}
