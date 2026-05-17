//! Extensible condition evaluation for automation tasks and event rules.
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
//! Condition expressions support:
//! - an array of conditions: AND logic
//! - `{"type":"all","conditions":[...]}`
//! - `{"type":"any","conditions":[...]}`
//! - `{"type":"not","condition":{...}}`
//!
//! Unknown condition types fail closed.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct ConditionContext {
    pub now: DateTime<Utc>,
    pub event: Option<serde_json::Value>,
}

impl ConditionContext {
    pub fn new(event: Option<serde_json::Value>) -> Self {
        Self {
            now: Utc::now(),
            event,
        }
    }
}

/// A single condition evaluator that handles one or more condition `type` values.
#[async_trait]
pub trait ConditionEvaluator: Send + Sync {
    /// The condition type string this evaluator handles (e.g. "sensor", "time_range").
    fn condition_type(&self) -> &'static str;

    /// Evaluate whether the condition is met.
    ///
    /// `condition` is the full JSON object from the task's conditions array.
    /// Returns `true` if the condition is satisfied.
    async fn evaluate(
        &self,
        condition: &serde_json::Value,
        ctx: &ConditionContext,
    ) -> anyhow::Result<bool>;
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

    /// Evaluate a condition expression. Empty/null conditions always pass.
    pub async fn evaluate_all(
        &self,
        conditions: &serde_json::Value,
        ctx: &ConditionContext,
    ) -> anyhow::Result<bool> {
        self.evaluate_expr(conditions, ctx).await
    }

    async fn evaluate_expr(
        &self,
        expr: &serde_json::Value,
        ctx: &ConditionContext,
    ) -> anyhow::Result<bool> {
        let mut stack = vec![EvalFrame::Enter(expr.clone())];
        let mut values = Vec::<bool>::new();

        while let Some(frame) = stack.pop() {
            match frame {
                EvalFrame::Enter(value) => match value {
                    serde_json::Value::Null => values.push(true),
                    serde_json::Value::Array(items) => {
                        stack.push(EvalFrame::Combine {
                            op: LogicalOp::All,
                            count: items.len(),
                        });
                        for item in items.into_iter().rev() {
                            stack.push(EvalFrame::Enter(item));
                        }
                    }
                    serde_json::Value::Object(map) => {
                        let cond_type = map.get("type").and_then(|v| v.as_str()).unwrap_or("all");
                        match cond_type {
                            "all" | "and" => {
                                let items = map
                                    .get("conditions")
                                    .and_then(|v| v.as_array())
                                    .cloned()
                                    .unwrap_or_default();
                                stack.push(EvalFrame::Combine {
                                    op: LogicalOp::All,
                                    count: items.len(),
                                });
                                for item in items.into_iter().rev() {
                                    stack.push(EvalFrame::Enter(item));
                                }
                            }
                            "any" | "or" => {
                                let items = map
                                    .get("conditions")
                                    .and_then(|v| v.as_array())
                                    .cloned()
                                    .unwrap_or_default();
                                stack.push(EvalFrame::Combine {
                                    op: LogicalOp::Any,
                                    count: items.len(),
                                });
                                for item in items.into_iter().rev() {
                                    stack.push(EvalFrame::Enter(item));
                                }
                            }
                            "not" => {
                                let item = map
                                    .get("condition")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null);
                                stack.push(EvalFrame::Combine {
                                    op: LogicalOp::Not,
                                    count: 1,
                                });
                                stack.push(EvalFrame::Enter(item));
                            }
                            "always" => values.push(true),
                            _ => {
                                let evaluators = self.evaluators.read().await;
                                let Some(evaluator) = evaluators.get(cond_type).cloned() else {
                                    tracing::warn!(
                                        condition_type = cond_type,
                                        "no evaluator registered for condition type"
                                    );
                                    values.push(false);
                                    continue;
                                };
                                drop(evaluators);
                                values.push(
                                    evaluator
                                        .evaluate(&serde_json::Value::Object(map), ctx)
                                        .await?,
                                );
                            }
                        }
                    }
                    _ => values.push(false),
                },
                EvalFrame::Combine { op, count } => {
                    if values.len() < count {
                        anyhow::bail!("condition evaluation stack underflow");
                    }
                    let start = values.len() - count;
                    let result = match op {
                        LogicalOp::All => values[start..].iter().all(|v| *v),
                        LogicalOp::Any => values[start..].iter().any(|v| *v),
                        LogicalOp::Not => !values[start],
                    };
                    values.truncate(start);
                    values.push(result);
                }
            }
        }

        Ok(values.pop().unwrap_or(true))
    }

    /// List registered condition type names.
    pub async fn registered_types(&self) -> Vec<String> {
        self.evaluators.read().await.keys().cloned().collect()
    }
}

enum EvalFrame {
    Enter(serde_json::Value),
    Combine { op: LogicalOp, count: usize },
}

enum LogicalOp {
    All,
    Any,
    Not,
}

impl Default for ConditionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
