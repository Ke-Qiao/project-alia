//! Built-in condition evaluators: sensor, time range, and event fields.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Timelike;

use super::condition::{ConditionContext, ConditionEvaluator};
use crate::providers::sensor::SensorRegistry;

/// Evaluates sensor-based conditions.
///
/// Example:
/// ```json
/// {"type": "sensor", "sensor_id": "room:temperature", "op": "gt", "value": 25.0}
/// ```
pub struct SensorConditionEvaluator {
    sensors: Arc<SensorRegistry>,
}

impl SensorConditionEvaluator {
    pub fn new(sensors: Arc<SensorRegistry>) -> Self {
        Self { sensors }
    }
}

#[async_trait]
impl ConditionEvaluator for SensorConditionEvaluator {
    fn condition_type(&self) -> &'static str {
        "sensor"
    }

    async fn evaluate(
        &self,
        condition: &serde_json::Value,
        _ctx: &ConditionContext,
    ) -> anyhow::Result<bool> {
        let Some(sensor_id) = condition.get("sensor_id").and_then(|v| v.as_str()) else {
            return Ok(false);
        };
        let Some(expected) = condition.get("value") else {
            return Ok(false);
        };
        let op = condition.get("op").and_then(|v| v.as_str()).unwrap_or("eq");

        let Some(reading) = self.sensors.latest(sensor_id).await else {
            return Ok(false);
        };

        Ok(compare_values(&reading.value, op, expected))
    }
}

/// Evaluates fields from the current event context.
///
/// Example:
/// ```json
/// {"type":"event_field","field":"payload.sensor_id","op":"eq","value":"room:temperature"}
/// ```
pub struct EventFieldConditionEvaluator;

#[async_trait]
impl ConditionEvaluator for EventFieldConditionEvaluator {
    fn condition_type(&self) -> &'static str {
        "event_field"
    }

    async fn evaluate(
        &self,
        condition: &serde_json::Value,
        ctx: &ConditionContext,
    ) -> anyhow::Result<bool> {
        let Some(event) = &ctx.event else {
            return Ok(false);
        };
        let Some(field) = condition.get("field").and_then(|v| v.as_str()) else {
            return Ok(false);
        };
        let op = condition.get("op").and_then(|v| v.as_str()).unwrap_or("eq");
        let expected = condition.get("value").unwrap_or(&serde_json::Value::Null);
        let actual = get_field(event, field);

        Ok(compare_values(&actual, op, expected))
    }
}

/// Evaluates event JSON by structural pattern.
///
/// Example:
/// ```json
/// {"type":"event_pattern","pattern":{"type":"SensorDataIngested"}}
/// ```
pub struct EventPatternConditionEvaluator;

#[async_trait]
impl ConditionEvaluator for EventPatternConditionEvaluator {
    fn condition_type(&self) -> &'static str {
        "event_pattern"
    }

    async fn evaluate(
        &self,
        condition: &serde_json::Value,
        ctx: &ConditionContext,
    ) -> anyhow::Result<bool> {
        let Some(event) = &ctx.event else {
            return Ok(false);
        };
        let pattern = condition
            .get("pattern")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        Ok(matches_pattern(event, &pattern))
    }
}

/// Evaluates time-of-day range conditions.
///
/// Example:
/// ```json
/// {"type": "time_range", "after": "08:00", "before": "22:00"}
/// ```
pub struct TimeRangeConditionEvaluator;

#[async_trait]
impl ConditionEvaluator for TimeRangeConditionEvaluator {
    fn condition_type(&self) -> &'static str {
        "time_range"
    }

    async fn evaluate(
        &self,
        condition: &serde_json::Value,
        ctx: &ConditionContext,
    ) -> anyhow::Result<bool> {
        let now_minutes = ctx.now.hour() * 60 + ctx.now.minute();

        let after_minutes = condition
            .get("after")
            .and_then(|v| v.as_str())
            .and_then(parse_hhmm);

        let before_minutes = condition
            .get("before")
            .and_then(|v| v.as_str())
            .and_then(parse_hhmm);

        Ok(match (after_minutes, before_minutes) {
            (Some(after), Some(before)) if after <= before => {
                now_minutes >= after && now_minutes < before
            }
            (Some(after), Some(before)) => now_minutes >= after || now_minutes < before,
            (Some(after), None) => now_minutes >= after,
            (None, Some(before)) => now_minutes < before,
            (None, None) => true,
        })
    }
}

fn compare_values(actual: &serde_json::Value, op: &str, expected: &serde_json::Value) -> bool {
    match op {
        "eq" => actual == expected,
        "neq" => actual != expected,
        "contains" => actual
            .as_str()
            .zip(expected.as_str())
            .is_some_and(|(actual, expected)| actual.contains(expected)),
        "gt" | "gte" | "lt" | "lte" => {
            let Some(actual) = value_as_f64(actual) else {
                return false;
            };
            let Some(expected) = value_as_f64(expected) else {
                return false;
            };
            match op {
                "gt" => actual > expected,
                "gte" => actual >= expected,
                "lt" => actual < expected,
                "lte" => actual <= expected,
                _ => false,
            }
        }
        _ => {
            tracing::warn!(op, "unknown condition operator");
            false
        }
    }
}

fn value_as_f64(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn get_field(data: &serde_json::Value, path: &str) -> serde_json::Value {
    let mut current = data;
    for part in path.split('.') {
        match current {
            serde_json::Value::Object(map) => {
                current = map.get(part).unwrap_or(&serde_json::Value::Null);
            }
            _ => return serde_json::Value::Null,
        }
    }
    current.clone()
}

fn matches_pattern(event: &serde_json::Value, pattern: &serde_json::Value) -> bool {
    match (event, pattern) {
        (_, serde_json::Value::Null) => true,
        (serde_json::Value::Object(ev), serde_json::Value::Object(pat)) => pat
            .iter()
            .all(|(k, v)| ev.get(k).is_some_and(|ev_v| matches_pattern(ev_v, v))),
        (ev, pat) => ev == pat,
    }
}

/// Parse "HH:MM" string into minutes since midnight.
fn parse_hhmm(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let h: u32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    if h >= 24 || m >= 60 {
        return None;
    }
    Some(h * 60 + m)
}
