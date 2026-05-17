//! Scheduler module: time-based, polling, event subscription, and action automation.
//!
//! This module provides:
//! - `AutomationScheduler`: cron, one-time, interval, polling-condition, and event subscriptions
//! - `ConditionRegistry` + `ConditionEvaluator`: Extensible condition evaluation
//!   (built-in: sensor, time_range; plugins can register custom evaluators at runtime)
//! - `ActionRegistry` + `ActionExecutor`: Shared action execution for scheduled tasks and rules

pub mod action;
pub mod condition;
pub mod cron;
pub mod evaluators;

pub use crate::providers::adapter::{ActionContext, ActionExecutor};
pub use action::ActionRegistry;
pub use condition::ConditionRegistry;
// Re-exported for plugin implementors
#[allow(unused_imports)]
pub use condition::ConditionEvaluator;
pub use cron::AutomationScheduler;
