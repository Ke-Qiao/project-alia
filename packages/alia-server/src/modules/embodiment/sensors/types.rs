//! Sensor values, freshness policy, and on-demand read capability.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use utoipa::{IntoParams, ToSchema};

/// Metadata describing a registered sensor.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SensorDefinition {
    /// Unique sensor identifier, for example "room:temperature" or "plugin.weather:humidity".
    pub sensor_id: String,
    /// Human-readable name
    pub display_name: String,
    /// Sensor source, such as "component:<name>", "plugin:<name>", "api", or "manual".
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
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, Default, Deserialize, IntoParams, ToSchema)]
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
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SensorSummary {
    pub sensor_id: String,
    pub display_name: String,
    pub unit: String,
    pub latest_value: Option<serde_json::Value>,
    pub latest_timestamp: Option<DateTime<Utc>>,
    pub reading_count: usize,
    pub online: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorReadMode {
    CacheOnly,
    BestEffort,
    Fresh,
}

impl SensorReadMode {
    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cache_only" | "cache-only" | "cache" | "latest" => Some(Self::CacheOnly),
            "best_effort" | "best-effort" | "best" | "auto" => Some(Self::BestEffort),
            "fresh" | "force_fresh" | "force-fresh" | "remote" => Some(Self::Fresh),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorStalePolicy {
    AllowStale,
    FailClosed,
}

impl SensorStalePolicy {
    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "allow_stale" | "allow-stale" | "allow" | "stale_ok" | "stale-ok" => {
                Some(Self::AllowStale)
            }
            "fail_closed" | "fail-closed" | "fresh_only" | "fresh-only" | "strict" => {
                Some(Self::FailClosed)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SensorFreshnessPolicy {
    pub mode: SensorReadMode,
    pub max_age: Duration,
    pub timeout: Duration,
    pub stale_policy: SensorStalePolicy,
}

impl SensorFreshnessPolicy {
    pub fn tool_default(offline_threshold: Duration) -> Self {
        Self {
            mode: SensorReadMode::BestEffort,
            max_age: offline_threshold,
            timeout: Duration::from_secs(2),
            stale_policy: SensorStalePolicy::AllowStale,
        }
    }

    pub fn scheduler_default() -> Self {
        Self {
            mode: SensorReadMode::Fresh,
            max_age: Duration::from_secs(5),
            timeout: Duration::from_secs(2),
            stale_policy: SensorStalePolicy::FailClosed,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SensorReadResult {
    pub definition: Option<SensorDefinition>,
    pub reading: Option<SensorReading>,
    pub fresh: bool,
    pub stale: bool,
    pub refreshed: bool,
    pub age_ms: Option<i64>,
    pub warning: Option<String>,
}

/// Pull-based reader for sensors that can be queried on demand.
///
/// Push-only sources can ignore this trait and keep using `ingest`. Channel
/// devices and future plugins can register a provider so generic tools and
/// scheduler conditions get fresh values without knowing channel internals.
#[async_trait]
pub trait SensorReadProvider: Send + Sync {
    fn source(&self) -> &str;

    async fn read_sensor(
        &self,
        sensor_id: &str,
        definition: &SensorDefinition,
    ) -> anyhow::Result<SensorReading>;
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
            retention_duration: Duration::from_secs(24 * 3600),
            cleanup_interval_sec: 300,
            offline_threshold: Duration::from_secs(120),
        }
    }
}
