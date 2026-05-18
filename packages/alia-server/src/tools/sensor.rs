//! Sensor tools for LLM function calling.
//!
//! - `sensor_read`: Read the latest value from a sensor
//! - `sensor_list`: List all registered sensors with their current status

use std::sync::Arc;

use async_trait::async_trait;

use crate::providers::adapter::{ToolContext, ToolDefinition, ToolExecutor, ToolResult};
use crate::providers::sensor::SensorRegistry;

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
            description: "Read the latest value from a sensor. Returns the most recent reading including value, timestamp, and unit.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "sensor_id": {
                        "type": "string",
                        "description": "The sensor ID to read (e.g., 'embedded/temperature', 'plugin.weather/humidity')"
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

        let reading = self.sensors.latest(sensor_id).await?;
        let def = self.sensors.get_sensor(sensor_id).await?;

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
                    "message": "no readings available"
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
