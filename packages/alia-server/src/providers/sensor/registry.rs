//! In-memory sensor registry with time-series storage and auto-cleanup.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// Metadata describing a registered sensor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorDefinition {
    /// Unique sensor identifier: "{source}/{name}" (e.g., "embedded/temperature", "plugin.weather/humidity")
    pub sensor_id: String,
    /// Human-readable name
    pub display_name: String,
    /// Sensor source: "embedded", "plugin", "api"
    pub source: String,
    /// Unit of measurement (e.g., "°C", "%", "lux", "boolean")
    pub unit: String,
    /// Data type hint: "float", "int", "bool", "string"
    pub value_type: String,
    /// Optional metadata (location, device_id, etc.)
    pub metadata: serde_json::Value,
    /// When the sensor was first registered
    pub registered_at: DateTime<Utc>,
    /// Last time a reading was received
    pub last_seen_at: Option<DateTime<Utc>>,
}

/// A single sensor reading (time-series data point).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorReading {
    pub sensor_id: String,
    pub value: serde_json::Value,
    pub timestamp: DateTime<Utc>,
    /// Optional quality/confidence indicator (0.0 - 1.0)
    #[serde(default = "default_quality")]
    pub quality: f32,
}

fn default_quality() -> f32 {
    1.0
}

/// Query filter for time-series data.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SensorQuery {
    pub sensor_id: Option<String>,
    pub source: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
    /// If true, return only the latest reading per sensor
    #[serde(default)]
    pub latest_only: bool,
}

/// Aggregated sensor summary.
#[derive(Debug, Clone, Serialize)]
pub struct SensorSummary {
    pub sensor_id: String,
    pub display_name: String,
    pub unit: String,
    pub latest_value: Option<serde_json::Value>,
    pub latest_timestamp: Option<DateTime<Utc>>,
    pub reading_count: usize,
    pub online: bool,
}

/// Configuration for the sensor registry.
#[derive(Debug, Clone)]
pub struct SensorRegistryConfig {
    /// Maximum readings to keep in memory per sensor
    pub max_readings_per_sensor: usize,
    /// Auto-cleanup: discard readings older than this duration
    pub retention_duration: Duration,
    /// How often to run cleanup (seconds)
    pub cleanup_interval_sec: u64,
    /// Sensor is considered offline if no reading for this duration
    pub offline_threshold: Duration,
}

impl Default for SensorRegistryConfig {
    fn default() -> Self {
        Self {
            max_readings_per_sensor: 1000,
            retention_duration: Duration::from_secs(24 * 3600), // 24h
            cleanup_interval_sec: 300,                          // 5 min
            offline_threshold: Duration::from_secs(120),        // 2 min
        }
    }
}

/// In-memory sensor registry with time-series ring buffer and auto-cleanup.
pub struct SensorRegistry {
    config: SensorRegistryConfig,
    /// Registered sensor definitions
    sensors: Arc<RwLock<HashMap<String, SensorDefinition>>>,
    /// Time-series data: sensor_id → Vec of readings (newest last)
    readings: Arc<RwLock<HashMap<String, Vec<SensorReading>>>>,
}

impl SensorRegistry {
    pub fn new(config: SensorRegistryConfig) -> Self {
        Self {
            config,
            sensors: Arc::new(RwLock::new(HashMap::new())),
            readings: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register or update a sensor definition.
    pub async fn register_sensor(&self, mut def: SensorDefinition) {
        let mut sensors = self.sensors.write().await;
        if sensors.contains_key(&def.sensor_id) {
            // Update last_seen, keep original registered_at
            if let Some(existing) = sensors.get_mut(&def.sensor_id) {
                existing.last_seen_at = Some(Utc::now());
                existing.display_name = def.display_name;
                existing.metadata = def.metadata;
            }
        } else {
            def.registered_at = Utc::now();
            tracing::info!(sensor_id = %def.sensor_id, source = %def.source, "sensor registered");
            sensors.insert(def.sensor_id.clone(), def);
        }
    }

    /// Unregister a sensor and discard its readings.
    pub async fn unregister_sensor(&self, sensor_id: &str) -> bool {
        let removed = self.sensors.write().await.remove(sensor_id).is_some();
        if removed {
            self.readings.write().await.remove(sensor_id);
            tracing::info!(%sensor_id, "sensor unregistered");
        }
        removed
    }

    /// Ingest a new reading. Auto-registers sensor if not already known.
    pub async fn ingest(&self, reading: SensorReading) {
        let sensor_id = reading.sensor_id.clone();

        // Auto-register if unknown
        {
            let sensors = self.sensors.read().await;
            if !sensors.contains_key(&sensor_id) {
                drop(sensors);
                self.register_sensor(SensorDefinition {
                    sensor_id: sensor_id.clone(),
                    display_name: sensor_id.clone(),
                    source: "unknown".into(),
                    unit: "".into(),
                    value_type: "float".into(),
                    metadata: serde_json::Value::Null,
                    registered_at: Utc::now(),
                    last_seen_at: Some(Utc::now()),
                })
                .await;
            } else {
                drop(sensors);
                // Update last_seen
                let mut sensors = self.sensors.write().await;
                if let Some(def) = sensors.get_mut(&sensor_id) {
                    def.last_seen_at = Some(Utc::now());
                }
            }
        }

        // Store reading
        let mut readings = self.readings.write().await;
        let series = readings.entry(sensor_id).or_default();
        series.push(reading);

        // Cap size
        if series.len() > self.config.max_readings_per_sensor {
            let excess = series.len() - self.config.max_readings_per_sensor;
            series.drain(..excess);
        }
    }

    /// Get the latest reading for a sensor.
    pub async fn latest(&self, sensor_id: &str) -> Option<SensorReading> {
        let readings = self.readings.read().await;
        readings
            .get(sensor_id)
            .and_then(|series| series.last().cloned())
    }

    /// Get the latest value as f64 (for condition evaluation).
    pub async fn latest_value_f64(&self, sensor_id: &str) -> Option<f64> {
        self.latest(sensor_id).await.and_then(|r| match &r.value {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        })
    }

    /// Query readings with filters.
    pub async fn query(&self, q: &SensorQuery) -> Vec<SensorReading> {
        let readings = self.readings.read().await;
        let mut results = Vec::new();

        let sensor_ids: Vec<String> = if let Some(ref id) = q.sensor_id {
            vec![id.clone()]
        } else if let Some(ref source) = q.source {
            let sensors = self.sensors.read().await;
            sensors
                .values()
                .filter(|s| s.source == *source)
                .map(|s| s.sensor_id.clone())
                .collect()
        } else {
            readings.keys().cloned().collect()
        };

        for sid in &sensor_ids {
            if let Some(series) = readings.get(sid) {
                if q.latest_only {
                    if let Some(last) = series.last() {
                        results.push(last.clone());
                    }
                } else {
                    for r in series.iter().rev() {
                        if let Some(from) = q.from {
                            if r.timestamp < from {
                                break;
                            }
                        }
                        if let Some(to) = q.to {
                            if r.timestamp > to {
                                continue;
                            }
                        }
                        results.push(r.clone());
                        if let Some(limit) = q.limit {
                            if results.len() >= limit {
                                return results;
                            }
                        }
                    }
                }
            }
        }

        results
    }

    /// List all registered sensors with summary.
    pub async fn list_sensors(&self) -> Vec<SensorSummary> {
        let sensors = self.sensors.read().await;
        let readings = self.readings.read().await;
        let now = Utc::now();

        sensors
            .values()
            .map(|def| {
                let series = readings.get(&def.sensor_id);
                let latest = series.and_then(|s| s.last());
                let online = def.last_seen_at.map_or(false, |t| {
                    (now - t).to_std().unwrap_or(Duration::MAX) < self.config.offline_threshold
                });

                SensorSummary {
                    sensor_id: def.sensor_id.clone(),
                    display_name: def.display_name.clone(),
                    unit: def.unit.clone(),
                    latest_value: latest.map(|r| r.value.clone()),
                    latest_timestamp: latest.map(|r| r.timestamp),
                    reading_count: series.map_or(0, |s| s.len()),
                    online,
                }
            })
            .collect()
    }

    /// Get a specific sensor definition.
    pub async fn get_sensor(&self, sensor_id: &str) -> Option<SensorDefinition> {
        self.sensors.read().await.get(sensor_id).cloned()
    }

    /// Run cleanup: discard readings older than retention_duration.
    pub async fn cleanup(&self) {
        let cutoff = Utc::now() - chrono::Duration::from_std(self.config.retention_duration)
            .unwrap_or(chrono::Duration::hours(24));
        let mut readings = self.readings.write().await;
        let mut total_removed = 0usize;

        for series in readings.values_mut() {
            let before = series.len();
            series.retain(|r| r.timestamp >= cutoff);
            total_removed += before - series.len();
        }

        if total_removed > 0 {
            tracing::debug!(removed = total_removed, "sensor cleanup completed");
        }
    }

    /// Spawn the auto-cleanup background task.
    pub fn spawn_cleanup_loop(&self, cancel_token: CancellationToken) {
        let readings = self.readings.clone();
        let retention = self.config.retention_duration;
        let interval_sec = self.config.cleanup_interval_sec;
        let max_per_sensor = self.config.max_readings_per_sensor;

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(interval_sec));

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    _ = interval.tick() => {
                        let cutoff = Utc::now()
                            - chrono::Duration::from_std(retention)
                                .unwrap_or(chrono::Duration::hours(24));

                        let mut readings = readings.write().await;
                        for series in readings.values_mut() {
                            series.retain(|r| r.timestamp >= cutoff);
                            if series.len() > max_per_sensor {
                                let excess = series.len() - max_per_sensor;
                                series.drain(..excess);
                            }
                        }
                    }
                }
            }
        });
    }
}
