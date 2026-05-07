//! Rule engine for automation rules.
//!
//! Available implementations:
//! - `SimpleRuleEngine`: Basic condition-based rule evaluation
//! - `NoopRuleEngine`: No-op placeholder (no rules)

pub mod engine;

pub use engine::SimpleRuleEngine;

use async_trait::async_trait;

use crate::providers::adapter::{RuleAction, RuleEvaluator};
use crate::event::ServerEvent;
use crate::module::ModuleLifecycle;

pub struct NoopRuleEngine;

#[async_trait]
impl ModuleLifecycle for NoopRuleEngine {
    fn name(&self) -> &'static str {
        "noop_rules"
    }
}

#[async_trait]
impl RuleEvaluator for NoopRuleEngine {
    async fn evaluate(&self, _event: &ServerEvent) -> anyhow::Result<Vec<RuleAction>> {
        Ok(Vec::new())
    }

    async fn reload(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
