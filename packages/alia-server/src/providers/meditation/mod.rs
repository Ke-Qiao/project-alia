#[cfg(feature = "meditation")]
mod scheduled;

use async_trait::async_trait;
use uuid::Uuid;

use crate::providers::adapter::{MeditationPhase, MeditationStatus, MeditationStrategy, MeditationTask};
use crate::module::ModuleLifecycle;

#[cfg(feature = "meditation")]
pub use scheduled::ScheduledMeditation;

pub struct NoopMeditation;

#[async_trait]
impl ModuleLifecycle for NoopMeditation {
    fn name(&self) -> &'static str {
        "noop"
    }
}

#[async_trait]
impl MeditationStrategy for NoopMeditation {
    async fn schedule(&self, _task: MeditationTask) -> anyhow::Result<()> {
        Ok(())
    }

    async fn status(&self, _user_id: Uuid) -> anyhow::Result<MeditationStatus> {
        Ok(MeditationStatus {
            enabled: false,
            last_phase: None,
            last_executed_at: None,
            pending_tasks: 0,
        })
    }

    async fn trigger(&self, _user_id: Uuid, _phase: MeditationPhase) -> anyhow::Result<()> {
        Ok(())
    }

    async fn get_conditions(&self) -> Vec<serde_json::Value> {
        vec![]
    }

    async fn set_conditions(&self, _conditions: Vec<serde_json::Value>) -> anyhow::Result<()> {
        Ok(())
    }
}
