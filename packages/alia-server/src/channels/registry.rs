use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::module::lifecycle::ModuleHealth;

use super::adapter::{ChannelAdapter, ChannelTarget};

/// Channel registry — manages all active channel adapters.
///
/// Supports multi-instance (unlike single-slot ModuleRegistry),
/// dynamic register/unregister, and per-ID addressing.
pub struct ChannelRegistry {
    channels: RwLock<HashMap<String, Arc<dyn ChannelAdapter>>>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
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
        map.insert(id, channel);
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
