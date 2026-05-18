//! Sensor tools for LLM function calling.
//!
//! - `sensor_read`: Read the latest value from a sensor
//! - `sensor_list`: List all registered sensors with their current status

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::providers::adapter::{ToolContext, ToolDefinition, ToolExecutor, ToolResult};
use crate::providers::sensor::{
    SensorFreshnessPolicy, SensorReadMode, SensorRegistry, SensorStalePolicy,
};

/// Tool: read the latest value from a sensor.
pub struct SensorReadTool {
    sensors: Arc<SensorRegistry>,
}

impl SensorReadTool {
    pub fn new(sensors: Arc<SensorRegistry>) -> Self {
        Self { sensors }
    }
}

#[async_trait]
impl ToolExecutor for SensorReadTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "sensor_read".to_string(),
            description: "Read a sensor value. Uses fresh/on-demand reads when available, with stale cache fallback by default.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "sensor_id": {
                        "type": "string",
                        "description": "The sensor ID to read (e.g., 'embedded/temperature', 'plugin.weather/humidity')"
                    },
                    "read_mode": {
                        "type": "string",
                        "enum": ["best_effort", "fresh", "cache_only"],
                        "description": "best_effort returns cache if fresh and falls back to stale data on remote failure; fresh tries the source reader when cache is old/missing; cache_only never calls the source reader."
                    },
                    "max_age_sec": {
                        "type": "number",
                        "description": "Maximum cache age accepted as fresh before trying an on-demand read."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout for an on-demand sensor read."
                    },
                    "stale_policy": {
                        "type": "string",
                        "enum": ["allow_stale", "fail_closed"],
                        "description": "Whether stale cached data is acceptable if a fresh read fails."
                    }
                },
                "required": ["sensor_id"]
            }),
            source: "builtin".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let sensor_id = arguments
            .get("sensor_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("sensor_id is required"))?;

        let policy = tool_freshness_policy(&arguments, self.sensors.offline_threshold());
        let result = self.sensors.read(sensor_id, policy.clone()).await?;
        let def = result.definition.clone();
        let reading =
            if result.stale && matches!(policy.stale_policy, SensorStalePolicy::FailClosed) {
                None
            } else {
                result.reading.clone()
            };
        let warning = result.warning.clone();

        match reading {
            Some(r) => {
                let unit = def.map(|d| d.unit).unwrap_or_default();
                Ok(ToolResult {
                    call_id: String::new(),
                    success: true,
                    output: serde_json::json!({
                        "sensor_id": sensor_id,
                        "value": r.value,
                        "unit": unit,
                        "timestamp": r.timestamp.to_rfc3339(),
                        "quality": r.quality,
                        "fresh": result.fresh,
                        "stale": result.stale,
                        "refreshed": result.refreshed,
                        "age_ms": result.age_ms,
                        "warning": warning,
                    }),
                    display: Some(format!("{}: {} {}", sensor_id, r.value, unit)),
                })
            }
            None => Ok(ToolResult {
                call_id: String::new(),
                success: true,
                output: serde_json::json!({
                    "sensor_id": sensor_id,
                    "value": null,
                    "fresh": result.fresh,
                    "stale": result.stale,
                    "refreshed": result.refreshed,
                    "age_ms": result.age_ms,
                    "warning": warning.clone(),
                    "message": warning.unwrap_or_else(|| "no readings available".to_string())
                }),
                display: Some(format!("{}: no data", sensor_id)),
            }),
        }
    }
}

/// Tool: list all registered sensors with their current status.
pub struct SensorListTool {
    sensors: Arc<SensorRegistry>,
}

impl SensorListTool {
    pub fn new(sensors: Arc<SensorRegistry>) -> Self {
        Self { sensors }
    }
}

fn tool_freshness_policy(
    arguments: &serde_json::Value,
    offline_threshold: Duration,
) -> SensorFreshnessPolicy {
    let mut policy = SensorFreshnessPolicy::tool_default(offline_threshold);
    if let Some(mode) = arguments
        .get("read_mode")
        .and_then(serde_json::Value::as_str)
        .and_then(SensorReadMode::from_str)
    {
        policy.mode = mode;
    }
    if let Some(stale_policy) = arguments
        .get("stale_policy")
        .and_then(serde_json::Value::as_str)
        .and_then(SensorStalePolicy::from_str)
    {
        policy.stale_policy = stale_policy;
    }
    if let Some(max_age_sec) = arguments
        .get("max_age_sec")
        .and_then(serde_json::Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
    {
        policy.max_age = Duration::from_millis((max_age_sec * 1000.0).round() as u64);
    }
    if let Some(timeout_ms) = arguments
        .get("timeout_ms")
        .and_then(serde_json::Value::as_u64)
        .filter(|value| *value > 0)
    {
        policy.timeout = Duration::from_millis(timeout_ms);
    }
    policy
}

#[async_trait]
impl ToolExecutor for SensorListTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "sensor_list".to_string(),
            description: "List all registered sensors with their current status, latest values, and online/offline state.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "Filter by source: 'embedded', 'plugin', 'api'. Omit to list all."
                    }
                }
            }),
            source: "builtin".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let source_filter = arguments.get("source").and_then(|v| v.as_str());
        let mut sensors = self.sensors.list_sensors().await?;

        if let Some(source) = source_filter {
            sensors.retain(|s| {
                s.sensor_id.starts_with(&format!("{}/", source))
                    || s.sensor_id.starts_with(&format!("{}:", source))
                    || s.sensor_id.starts_with(&format!("{}.", source))
            });
        }

        let count = sensors.len();
        Ok(ToolResult {
            call_id: String::new(),
            success: true,
            output: serde_json::json!({
                "sensors": sensors,
                "count": count,
            }),
            display: Some(format!("{} sensors registered", count)),
        })
    }
}
