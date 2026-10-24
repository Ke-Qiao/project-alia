//! Sensor tools for LLM function calling.
//!
//! - `sensor_read`: Read the latest value from a sensor
//! - `sensor_list`: List all registered sensors with their current status

use std::sync::Arc;
use std::time::Duration;

use crate::modules::embodiment::sensors::{
    SensorFreshnessPolicy, SensorReadMode, SensorRegistry, SensorStalePolicy,
};
use crate::providers::adapter::{ToolContext, ToolEntry, ToolResult};

/// Tool: read the latest value from a sensor.
pub struct SensorReadTool {
    sensors: Arc<SensorRegistry>,
}

impl SensorReadTool {
    pub fn new(sensors: Arc<SensorRegistry>) -> Self {
        Self { sensors }
    }
}

impl SensorReadTool {
    pub fn entry(self) -> anyhow::Result<Arc<ToolEntry>> {
        let revision = "alia.builtin.SensorReadTool";
        ToolEntry::from_tool(self, "builtin".to_string(), revision, false, None)
    }

    pub async fn execute(
        &self,
        arguments: SensorReadToolArgs,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let sensor_id = arguments.sensor_id.as_str();
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
                    presentation: None,
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
                presentation: None,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SensorReadToolArgs {
    #[schemars(length(min = 1, max = 512))]
    pub sensor_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_mode: Option<ReadMode>,
    #[schemars(range(min = 0, max = 86400))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_age_sec: Option<f64>,
    #[schemars(range(min = 1, max = 30000))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_policy: Option<StalePolicy>,
}

impl rig_core::tool::Tool for SensorReadTool {
    const NAME: &'static str = "sensor_read";
    type Args = SensorReadToolArgs;
    type Output = rig_core::tool::ToolOutput;
    type Error = rig_core::tool::ToolExecutionError;
    fn description(&self) -> String {
        "Read a sensor value. Uses fresh/on-demand reads when available, with stale cache fallback by default.".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        crate::tools::entry::parameters::<Self::Args>()
    }
    async fn call(
        &self,
        context: &mut rig_core::tool::ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let invocation = context
            .require::<ToolContext>()
            .map_err(crate::tools::entry::tool_error)?;
        self.execute(args, &invocation)
            .await
            .map_err(crate::tools::entry::tool_error)?
            .publish(context)
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
    arguments: &SensorReadToolArgs,
    offline_threshold: Duration,
) -> SensorFreshnessPolicy {
    let mut policy = SensorFreshnessPolicy::tool_default(offline_threshold);
    if let Some(mode) = &arguments.read_mode {
        policy.mode = match mode {
            ReadMode::BestEffort => SensorReadMode::BestEffort,
            ReadMode::Fresh => SensorReadMode::Fresh,
            ReadMode::CacheOnly => SensorReadMode::CacheOnly,
        };
    }
    if let Some(stale) = &arguments.stale_policy {
        policy.stale_policy = match stale {
            StalePolicy::AllowStale => SensorStalePolicy::AllowStale,
            StalePolicy::FailClosed => SensorStalePolicy::FailClosed,
        };
    }
    if let Some(seconds) = arguments.max_age_sec {
        policy.max_age = Duration::from_millis((seconds * 1000.0).round() as u64);
    }
    if let Some(milliseconds) = arguments.timeout_ms {
        policy.timeout = Duration::from_millis(milliseconds);
    }
    policy
}

impl SensorListTool {
    pub fn entry(self) -> anyhow::Result<Arc<ToolEntry>> {
        let revision = "alia.builtin.SensorListTool";
        ToolEntry::from_tool(self, "builtin".to_string(), revision, false, None)
    }

    pub async fn execute(
        &self,
        arguments: SensorListToolArgs,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolResult> {
        let source_filter = arguments.source.as_deref();
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
            presentation: None,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SensorListToolArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl rig_core::tool::Tool for SensorListTool {
    const NAME: &'static str = "sensor_list";
    type Args = SensorListToolArgs;
    type Output = rig_core::tool::ToolOutput;
    type Error = rig_core::tool::ToolExecutionError;
    fn description(&self) -> String {
        "List all registered sensors with their current status, latest values, and online/offline state.".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        crate::tools::entry::parameters::<Self::Args>()
    }
    async fn call(
        &self,
        context: &mut rig_core::tool::ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let invocation = context
            .require::<ToolContext>()
            .map_err(crate::tools::entry::tool_error)?;
        self.execute(args, &invocation)
            .await
            .map_err(crate::tools::entry::tool_error)?
            .publish(context)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    BestEffort,
    Fresh,
    CacheOnly,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StalePolicy {
    AllowStale,
    FailClosed,
}
