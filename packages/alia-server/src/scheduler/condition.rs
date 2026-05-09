//! Extensible condition evaluation for scheduled tasks.
//!
//! Conditions are JSON objects with a `"type"` field that identifies which evaluator handles them.
//! Plugins can register custom evaluators via `ConditionRegistry::register()`.
//!
//! ## Built-in condition types
//!
//! - `"sensor"`: Check sensor readings against thresholds
//!   ```json
//!   {"type": "sensor", "sensor_id": "room:temperature", "op": "gt", "value": 25.0}
//!   ```
//! - `"time_range"`: Check current time is within a window
//!   ```json
//!   {"type": "time_range", "after": "08:00", "before": "22:00"}
//!   ```
//!
//! ## Semantics
//!
//! All conditions in a task's `conditions` array must pass (AND logic).
//! If no evaluator is registered for a condition type, it is treated as passing (open-world).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

/// A single condition evaluator that handles one or more condition `type` values.
#[async_trait]
pub trait ConditionEvaluator: Send + Sync {
    /// The condition type string this evaluator handles (e.g. "sensor", "time_range").
    fn condition_type(&self) -> &'static str;

    /// Evaluate whether the condition is met.
    ///
    /// `condition` is the full JSON object from the task's conditions array.
    /// Returns `true` if the condition is satisfied.
    async fn evaluate(&self, condition: &serde_json::Value) -> bool;
}

/// Registry of condition evaluators, keyed by condition type.
///
/// Thread-safe and supports runtime registration (plugins can add evaluators after startup).
pub struct ConditionRegistry {
    evaluators: Arc<RwLock<HashMap<String, Arc<dyn ConditionEvaluator>>>>,
}

impl ConditionRegistry {
    pub fn new() -> Self {
        Self {
            evaluators: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a condition evaluator. Replaces any existing evaluator for the same type.
    pub async fn register(&self, evaluator: Arc<dyn ConditionEvaluator>) {
        let cond_type = evaluator.condition_type().to_string();
        tracing::info!(condition_type = %cond_type, "registered condition evaluator");
        self.evaluators.write().await.insert(cond_type, evaluator);
    }

    /// Evaluate all conditions in a JSON array (AND logic).
    ///
    /// Returns `true` if ALL conditions pass. Empty/null conditions always pass.
    /// Conditions whose type has no registered evaluator are treated as passing.
    pub async fn evaluate_all(&self, conditions: &serde_json::Value) -> bool {
        let Some(arr) = conditions.as_array() else {
            return true;
        };
        if arr.is_empty() {
            return true;
        }

        let evaluators = self.evaluators.read().await;

        for cond in arr {
            let Some(cond_type) = cond.get("type").and_then(|v| v.as_str()) else {
                // Malformed condition (no type) — skip
                continue;
            };

            let Some(evaluator) = evaluators.get(cond_type) else {
                // No evaluator registered for this type — open-world, pass
                tracing::trace!(condition_type = cond_type, "no evaluator registered, skipping");
                continue;
            };

            if !evaluator.evaluate(cond).await {
                return false;
            }
        }

        true
    }

    /// List registered condition type names.
    pub async fn registered_types(&self) -> Vec<String> {
        self.evaluators.read().await.keys().cloned().collect()
    }
}

impl Default for ConditionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
