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

    /// Resolve a channel identifier.
    ///
    /// Exact instance IDs always win. If no exact ID exists, a value that
    /// matches exactly one channel type is treated as a legacy/type alias and
    /// resolved to that channel's concrete instance ID.
    pub async fn resolve_id_or_type(&self, id_or_type: &str) -> anyhow::Result<Option<String>> {
        let requested = id_or_type.trim();
        if requested.is_empty() {
            return Ok(None);
        }

        let channels = self.channels.read().await;
        if channels.contains_key(requested) {
            return Ok(Some(requested.to_string()));
        }

        let matches = channels
            .values()
            .filter(|channel| channel.channel_type() == requested)
            .map(|channel| channel.channel_id().to_string())
            .collect::<Vec<_>>();

        match matches.as_slice() {
            [] => Ok(None),
            [channel_id] => {
                tracing::warn!(
                    requested,
                    resolved_channel_id = %channel_id,
                    "channel id matched a channel type; resolved to concrete channel id"
                );
                Ok(Some(channel_id.clone()))
            }
            _ => anyhow::bail!(
                "channel id '{}' matched multiple channels by type; use a concrete channel id",
                requested
            ),
        }
    }

    /// Get a channel by exact ID or by unique channel type alias.
    pub async fn get_resolved(
        &self,
        id_or_type: &str,
    ) -> anyhow::Result<Option<(String, Arc<dyn ChannelAdapter>)>> {
        let Some(channel_id) = self.resolve_id_or_type(id_or_type).await? else {
            return Ok(None);
        };
        Ok(self
            .get(&channel_id)
            .await
            .map(|channel| (channel_id, channel)))
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
        let id = normalize_channel_identifier(channel.channel_id(), "channel id")?;
        let channel_type = normalize_channel_identifier(channel.channel_type(), "channel type")?;
        let mut map = self.channels.write().await;
        if map.contains_key(&id) {
            anyhow::bail!("channel '{}' already registered", id);
        }
        validate_channel_identity_not_ambiguous(&map, &id, &channel_type)?;
        tracing::info!(
            channel_type = %channel_type,
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
                voice: None,
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

fn normalize_channel_identifier(value: &str, label: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("{label} must not be empty");
    }
    Ok(value.to_string())
}

fn validate_channel_identity_not_ambiguous(
    channels: &HashMap<String, Arc<dyn ChannelAdapter>>,
    new_id: &str,
    new_type: &str,
) -> anyhow::Result<()> {
    for existing in channels.values() {
        let existing_id = existing.channel_id();
        let existing_type = existing.channel_type();
        if new_id == existing_type && new_type != existing_type {
            anyhow::bail!(
                "channel id '{}' conflicts with existing channel type '{}'; use a concrete instance id that is not another channel type",
                new_id,
                existing_type
            );
        }
        if new_type == existing_id && new_id != existing_id {
            anyhow::bail!(
                "channel type '{}' conflicts with existing channel id '{}'; use a channel id that does not shadow a channel type",
                new_type,
                existing_id
            );
        }
    }
    Ok(())
}
