use async_trait::async_trait;
use uuid::Uuid;

use crate::providers::adapter::{MeditationPhase, MeditationStatus, MeditationStrategy, MeditationTask};
use crate::module::ModuleLifecycle;

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
}
