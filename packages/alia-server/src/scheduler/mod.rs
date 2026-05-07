//! Scheduler module: Cron-based task scheduling and event subscriptions
//!
//! Provides two scheduling mechanisms:
//! - **Scheduled Tasks**: Time-based execution (cron/once/interval)
//! - **Event Subscriptions**: Trigger actions when EventBus events match patterns
//!
//! Both support conditions, cooldown periods, retries, and plugin-extensible actions.

use async_trait::async_trait;
use uuid::Uuid;

use crate::module::ModuleLifecycle;
use crate::providers::adapter::{
    ScheduledTask, TaskFilter, TaskPatch, EventSubscription, SchedulerBackend,
};

mod cron;
pub use cron::CronScheduler;

/// No-op scheduler: disables all scheduling features
pub struct NoopScheduler;

#[async_trait]
impl ModuleLifecycle for NoopScheduler {
    fn name(&self) -> &'static str {
        "noop_scheduler"
    }
}

#[async_trait]
impl SchedulerBackend for NoopScheduler {
    async fn create_task(&self, _task: ScheduledTask) -> anyhow::Result<Uuid> {
        Ok(Uuid::new_v4())
    }

    async fn update_task(&self, _task_id: Uuid, _patch: TaskPatch) -> anyhow::Result<()> {
        Ok(())
    }

    async fn delete_task(&self, _task_id: Uuid) -> anyhow::Result<()> {
        Ok(())
    }

    async fn list_tasks(&self, _filter: TaskFilter) -> anyhow::Result<Vec<ScheduledTask>> {
        Ok(Vec::new())
    }

    async fn pause_task(&self, _task_id: Uuid) -> anyhow::Result<()> {
        Ok(())
    }

    async fn resume_task(&self, _task_id: Uuid) -> anyhow::Result<()> {
        Ok(())
    }

    async fn subscribe_event(&self, _sub: EventSubscription) -> anyhow::Result<Uuid> {
        Ok(Uuid::new_v4())
    }

    async fn unsubscribe_event(&self, _sub_id: Uuid) -> anyhow::Result<()> {
        Ok(())
    }

    async fn list_subscriptions(&self, _user_id: Uuid) -> anyhow::Result<Vec<EventSubscription>> {
        Ok(Vec::new())
    }
}
