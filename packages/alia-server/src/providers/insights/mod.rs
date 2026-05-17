#[cfg(feature = "insights")]
mod scheduled;

use async_trait::async_trait;
use uuid::Uuid;

use crate::module::ModuleLifecycle;
use crate::providers::adapter::{InsightPhase, InsightStatus, InsightStrategy, InsightTask};

#[cfg(feature = "insights")]
pub use scheduled::ScheduledInsights;

pub struct NoopInsights;

#[async_trait]
impl ModuleLifecycle for NoopInsights {
    fn name(&self) -> &'static str {
        "noop"
    }
}

#[async_trait]
impl InsightStrategy for NoopInsights {
    async fn schedule(&self, _task: InsightTask) -> anyhow::Result<()> {
        Ok(())
    }

    async fn status(&self, _user_id: Uuid) -> anyhow::Result<InsightStatus> {
        Ok(InsightStatus {
            enabled: false,
            last_phase: None,
            last_executed_at: None,
            pending_tasks: 0,
        })
    }

    async fn trigger(&self, _user_id: Uuid, _phase: InsightPhase) -> anyhow::Result<()> {
        Ok(())
    }

    async fn get_conditions(&self) -> Vec<serde_json::Value> {
        vec![]
    }

    async fn set_conditions(&self, _conditions: Vec<serde_json::Value>) -> anyhow::Result<()> {
        Ok(())
    }
}
