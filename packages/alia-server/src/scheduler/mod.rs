//! Scheduler module: time-based task scheduling and event subscriptions.
//!
//! This module provides:
//! - `NoopScheduler`: A no-op implementation for disabled scheduler
//! - `CronScheduler`: A full implementation with cron, one-time, and interval scheduling

mod cron;

pub use cron::CronScheduler;

use async_trait::async_trait;
use uuid::Uuid;

use crate::module::ModuleLifecycle;
use crate::providers::adapter::{
    EventSubscription, ScheduledTask, SchedulerBackend, TaskFilter, TaskPatch,
};

/// No-op scheduler implementation for disabled state.
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
        anyhow::bail!("scheduler is disabled")
    }

    async fn update_task(&self, _task_id: Uuid, _patch: TaskPatch) -> anyhow::Result<()> {
        anyhow::bail!("scheduler is disabled")
    }

    async fn delete_task(&self, _task_id: Uuid) -> anyhow::Result<()> {
        anyhow::bail!("scheduler is disabled")
    }

    async fn list_tasks(&self, _filter: TaskFilter) -> anyhow::Result<Vec<ScheduledTask>> {
        Ok(Vec::new())
    }

    async fn pause_task(&self, _task_id: Uuid) -> anyhow::Result<()> {
        anyhow::bail!("scheduler is disabled")
    }

    async fn resume_task(&self, _task_id: Uuid) -> anyhow::Result<()> {
        anyhow::bail!("scheduler is disabled")
    }

    async fn subscribe_event(&self, _sub: EventSubscription) -> anyhow::Result<Uuid> {
        anyhow::bail!("scheduler is disabled")
    }

    async fn unsubscribe_event(&self, _sub_id: Uuid) -> anyhow::Result<()> {
        anyhow::bail!("scheduler is disabled")
    }

    async fn list_subscriptions(&self, _user_id: Uuid) -> anyhow::Result<Vec<EventSubscription>> {
        Ok(Vec::new())
    }
}
