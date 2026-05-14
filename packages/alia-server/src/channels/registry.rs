use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::RwLock;

use crate::module::lifecycle::ModuleHealth;

use super::adapter::{ChannelAdapter, ChannelTarget};

/// Channel registry — manages all active channel adapters.
///
/// Supports multi-instance (unlike single-slot ModuleRegistry),
/// dynamic register/unregister, and per-ID addressing.
pub struct ChannelRegistry {
    channels: RwLock<HashMap<String, Arc<dyn ChannelAdapter>>>,
    disabled: RwLock<HashMap<String, DisabledChannelInfo>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisabledChannelInfo {
    pub id: String,
    pub channel_type: String,
    pub reason: String,
    pub disabled_at: DateTime<Utc>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
            disabled: RwLock::new(HashMap::new()),
        }
    }

    /// Get a channel by its instance ID.
    pub async fn get(&self, id: &str) -> Option<Arc<dyn ChannelAdapter>> {
        self.channels.read().await.get(id).cloned()
    }

    /// List all active channels.
    pub async fn list(&self) -> Vec<Arc<dyn ChannelAdapter>> {
        self.channels.read().await.values().cloned().collect()
    }

    /// List disabled channels and their last failure reason.
    pub async fn disabled(&self) -> Vec<DisabledChannelInfo> {
        self.disabled.read().await.values().cloned().collect()
    }

    /// Register a channel (at startup or hot-add at runtime).
    pub async fn register(&self, channel: Arc<dyn ChannelAdapter>) -> anyhow::Result<()> {
        let id = channel.channel_id().to_string();
        let mut map = self.channels.write().await;
        if map.contains_key(&id) {
            anyhow::bail!("channel '{}' already registered", id);
        }
        tracing::info!(
            channel_type = channel.channel_type(),
            channel_id = %id,
            "channel registered"
        );
        map.insert(id.clone(), channel);
        self.disabled.write().await.remove(&id);
        Ok(())
    }

    /// Mark a channel disabled without requiring an active channel instance.
    pub async fn mark_disabled(&self, id: &str, channel_type: &str, reason: impl ToString) {
        self.disabled.write().await.insert(
            id.to_string(),
            DisabledChannelInfo {
                id: id.to_string(),
                channel_type: channel_type.to_string(),
                reason: reason.to_string(),
                disabled_at: Utc::now(),
            },
        );
    }

    /// Stop, unregister, and remember why a channel was disabled.
    pub async fn disable(&self, id: &str, reason: impl ToString) -> anyhow::Result<()> {
        let mut map = self.channels.write().await;
        let Some(channel) = map.remove(id) else {
            anyhow::bail!("channel '{}' not found", id);
        };
        let channel_type = channel.channel_type().to_string();
        drop(map);

        let reason = reason.to_string();
        if let Err(error) = channel.stop().await {
            tracing::warn!(
                channel_type = channel_type,
                channel_id = id,
                error = ?error,
                "channel stop failed while disabling"
            );
        }

        self.mark_disabled(id, &channel_type, reason).await;
        tracing::warn!(
            channel_type = channel_type,
            channel_id = id,
            "channel disabled"
        );
        Ok(())
    }

    /// Unregister a channel by ID.
    pub async fn unregister(&self, id: &str) -> anyhow::Result<()> {
        let mut map = self.channels.write().await;
        let channel = map
            .remove(id)
            .ok_or_else(|| anyhow::anyhow!("channel '{}' not found", id))?;
        tracing::info!(
            channel_type = channel.channel_type(),
            channel_id = id,
            "channel unregistered"
        );
        Ok(())
    }

    /// Broadcast a text message to all channels.
    pub async fn broadcast(&self, msg: &str) -> anyhow::Result<()> {
        let channels = self.channels.read().await;
        for channel in channels.values() {
            let target = ChannelTarget {
                user_id: String::new(),
            };
            if let Err(e) = channel.send_message(&target, msg).await {
                tracing::warn!(
                    channel_id = channel.channel_id(),
                    error = ?e,
                    "broadcast failed for channel"
                );
            }
        }
        Ok(())
    }

    /// Gracefully stop and shutdown all channels.
    pub async fn shutdown(&self) {
        let channels = self.channels.read().await;
        for channel in channels.values() {
            if let Err(e) = channel.stop().await {
                tracing::warn!(
                    channel_id = channel.channel_id(),
                    error = ?e,
                    "channel stop failed"
                );
            }
        }
    }

    /// Aggregate health status of all channels.
    pub async fn health(&self) -> HashMap<String, ModuleHealth> {
        let channels = self.channels.read().await;
        let mut results = HashMap::new();
        for (id, channel) in channels.iter() {
            results.insert(id.clone(), channel.health_check().await);
        }
        results
    }
}
