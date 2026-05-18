use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Static metadata for a sensor exposed by alia-embedded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SensorDefinition {
    pub sensor_id: String,
    pub display_name: String,
    pub source: String,
    pub unit: String,
    pub value_type: String,
    pub capabilities: Vec<String>,
    pub metadata: serde_json::Value,
}

impl SensorDefinition {
    pub fn new(sensor_id: impl Into<String>, display_name: impl Into<String>) -> Self {
        let sensor_id = sensor_id.into();
        Self {
            sensor_id: sensor_id.clone(),
            display_name: display_name.into(),
            source: "embedded".to_string(),
            unit: String::new(),
            value_type: "json".to_string(),
            capabilities: vec!["read".to_string()],
            metadata: serde_json::json!({}),
        }
    }
}

/// A live reading produced by a concrete sensor provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SensorReading {
    pub sensor_id: String,
    pub value: serde_json::Value,
    pub timestamp_us: u64,
    pub quality: f32,
}

impl SensorReading {
    pub fn now(sensor_id: impl Into<String>, value: serde_json::Value) -> Self {
        Self {
            sensor_id: sensor_id.into(),
            value,
            timestamp_us: timestamp_us(),
            quality: 1.0,
        }
    }
}

/// Concrete sensor implementation hook.
///
/// Future hardware/plugin modules should implement this trait and register
/// themselves with `ModuleContext::sensors`. The registry owns discovery and
/// routing; this trait owns device-specific read/control behavior.
#[tonic::async_trait]
pub trait SensorProvider: Send + Sync {
    fn definition(&self) -> SensorDefinition;

    async fn read(&self) -> Result<SensorReading>;

    async fn invoke(&self, command: &str, _args: serde_json::Value) -> Result<serde_json::Value> {
        anyhow::bail!(
            "sensor '{}' does not support command '{}'",
            self.definition().sensor_id,
            command
        )
    }
}

/// Runtime registry for embedded sensor providers.
#[derive(Default)]
pub struct SensorRegistry {
    providers: RwLock<HashMap<String, Arc<dyn SensorProvider>>>,
}

impl SensorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register or replace a sensor provider. Returns true when an existing
    /// provider for the same sensor id was replaced.
    pub async fn register(&self, provider: Arc<dyn SensorProvider>) -> Result<bool> {
        let definition = provider.definition();
        validate_sensor_id(&definition.sensor_id)?;
        let mut providers = self.providers.write().await;
        let replaced = providers
            .insert(definition.sensor_id.clone(), provider)
            .is_some();
        if replaced {
            tracing::warn!(
                sensor_id = %definition.sensor_id,
                "embedded sensor provider replaced"
            );
        } else {
            tracing::info!(
                sensor_id = %definition.sensor_id,
                source = %definition.source,
                "embedded sensor provider registered"
            );
        }
        Ok(replaced)
    }

    pub async fn unregister(&self, sensor_id: &str) -> Result<bool> {
        validate_sensor_id(sensor_id)?;
        let removed = self.providers.write().await.remove(sensor_id).is_some();
        if removed {
            tracing::info!(%sensor_id, "embedded sensor provider unregistered");
        }
        Ok(removed)
    }

    pub async fn list(&self) -> Vec<SensorDefinition> {
        let mut definitions = self
            .providers
            .read()
            .await
            .values()
            .map(|provider| provider.definition())
            .collect::<Vec<_>>();
        definitions.sort_by(|a, b| a.sensor_id.cmp(&b.sensor_id));
        definitions
    }

    pub async fn get(&self, sensor_id: &str) -> Result<Option<SensorDefinition>> {
        validate_sensor_id(sensor_id)?;
        Ok(self
            .providers
            .read()
            .await
            .get(sensor_id)
            .map(|provider| provider.definition()))
    }

    pub async fn read(&self, sensor_id: &str) -> Result<SensorReading> {
        let provider = self.provider(sensor_id).await?;
        let mut reading = provider.read().await?;
        reading.sensor_id = sensor_id.to_string();
        reading.quality = reading.quality.clamp(0.0, 1.0);
        Ok(reading)
    }

    pub async fn invoke(
        &self,
        sensor_id: &str,
        command: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let command = command.trim();
        if command.is_empty() {
            anyhow::bail!("sensor command must not be empty");
        }
        let provider = self.provider(sensor_id).await?;
        provider.invoke(command, args).await
    }

    pub async fn len(&self) -> usize {
        self.providers.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    async fn provider(&self, sensor_id: &str) -> Result<Arc<dyn SensorProvider>> {
        validate_sensor_id(sensor_id)?;
        self.providers
            .read()
            .await
            .get(sensor_id)
            .cloned()
            .with_context(|| format!("embedded sensor '{sensor_id}' is not registered"))
    }
}

fn validate_sensor_id(sensor_id: &str) -> Result<()> {
    let sensor_id = sensor_id.trim();
    if sensor_id.is_empty() || sensor_id.len() > 200 {
        anyhow::bail!("sensor_id must be between 1 and 200 characters");
    }
    Ok(())
}

fn timestamp_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros() as u64)
        .unwrap_or(0)
}
