//! Built-in condition evaluators: sensor and time_range.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Timelike, Utc};

use super::condition::ConditionEvaluator;
use crate::providers::sensor::SensorRegistry;

/// Evaluates sensor-based conditions.
///
/// Condition format:
/// ```json
/// {"type": "sensor", "sensor_id": "room:temperature", "op": "gt", "value": 25.0}
/// ```
///
/// Supported operators: `gt`, `gte`, `lt`, `lte`, `eq`
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

    async fn evaluate(&self, condition: &serde_json::Value) -> bool {
        let Some(sensor_id) = condition.get("sensor_id").and_then(|v| v.as_str()) else {
            return false;
        };
        let Some(op) = condition.get("op").and_then(|v| v.as_str()) else {
            return false;
        };
        let Some(threshold) = condition.get("value").and_then(|v| v.as_f64()) else {
            return false;
        };

        let Some(current) = self.sensors.latest_value_f64(sensor_id).await else {
            return false;
        };

        match op {
            "gt" => current > threshold,
            "gte" => current >= threshold,
            "lt" => current < threshold,
            "lte" => current <= threshold,
            "eq" => (current - threshold).abs() < f64::EPSILON,
            _ => {
                tracing::warn!(op, "unknown sensor condition operator");
                false
            }
        }
    }
}

/// Evaluates time-of-day range conditions.
///
/// Condition format:
/// ```json
/// {"type": "time_range", "after": "08:00", "before": "22:00"}
/// ```
///
/// Both `after` and `before` are optional. If only `after` is set, checks current >= after.
/// If only `before` is set, checks current < before.
/// If both are set and `after < before`, checks `after <= now < before`.
/// If both are set and `after > before` (overnight), checks `now >= after || now < before`.
pub struct TimeRangeConditionEvaluator;

#[async_trait]
impl ConditionEvaluator for TimeRangeConditionEvaluator {
    fn condition_type(&self) -> &'static str {
        "time_range"
    }

    async fn evaluate(&self, condition: &serde_json::Value) -> bool {
        let now = Utc::now();
        let now_minutes = now.hour() * 60 + now.minute();

        let after_minutes = condition
            .get("after")
            .and_then(|v| v.as_str())
            .and_then(parse_hhmm);

        let before_minutes = condition
            .get("before")
            .and_then(|v| v.as_str())
            .and_then(parse_hhmm);

        match (after_minutes, before_minutes) {
            (Some(after), Some(before)) if after <= before => {
                // Normal range: 08:00 - 22:00
                now_minutes >= after && now_minutes < before
            }
            (Some(after), Some(before)) => {
                // Overnight range: 22:00 - 06:00
                now_minutes >= after || now_minutes < before
            }
            (Some(after), None) => now_minutes >= after,
            (None, Some(before)) => now_minutes < before,
            (None, None) => true,
        }
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
