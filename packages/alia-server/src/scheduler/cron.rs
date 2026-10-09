//! AutomationScheduler: cron, one-shot, interval, condition polling, and event subscriptions.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cron::Schedule;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use tokio::sync::{Mutex, RwLock, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::action::{ActionRegistry, capture_action_sources};
use super::condition::{ConditionContext, ConditionRegistry};
use crate::config::SchedulerSection;
use crate::event::{EventBus, ServerEvent};
use crate::modules::platform::background::OwnedBackground;
use crate::modules::platform::lifecycle::{ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    ActionContext, EventSubscription, ScheduleConfig, ScheduledTask, SchedulerBackend, TaskFilter,
    TaskPatch,
};

#[derive(Debug, Clone)]
struct TaskState {
    task: ScheduledTask,
    next_check: Option<DateTime<Utc>>,
    last_checked: Option<DateTime<Utc>>,
    last_triggered: Option<DateTime<Utc>>,
    retry_count: u32,
    cron_schedule: Option<Schedule>,
}

impl TaskState {
    fn new(task: ScheduledTask) -> anyhow::Result<Self> {
        let cron_schedule = parse_cron_schedule(&task.schedule)?;
        let next_check = compute_next_check(&task.schedule, &cron_schedule, None)?;
        Ok(Self {
            task,
            next_check,
            last_checked: None,
            last_triggered: None,
            retry_count: 0,
            cron_schedule,
        })
    }

    fn refresh_schedule(&mut self) -> anyhow::Result<()> {
        self.cron_schedule = parse_cron_schedule(&self.task.schedule)?;
        self.next_check = compute_next_check(&self.task.schedule, &self.cron_schedule, None)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct SubscriptionState {
    sub: EventSubscription,
    last_triggered: Option<DateTime<Utc>>,
}

enum ActionMessage {
    Task {
        task_id: Uuid,
        request_id: Uuid,
        source_received_at: DateTime<Utc>,
        source_privacy_epoch: Option<i64>,
        source_user_id: Uuid,
    },
    Subscription {
        subscription_id: Uuid,
        request_id: Uuid,
        source_received_at: DateTime<Utc>,
        source_privacy_epoch: Option<i64>,
        source_user_id: Uuid,
        event: serde_json::Value,
    },
}

pub struct AutomationScheduler {
    background: OwnedBackground,
    initialized: bool,
    tasks: Arc<RwLock<HashMap<Uuid, TaskState>>>,
    subscriptions: Arc<RwLock<HashMap<Uuid, SubscriptionState>>>,
    action_tx: Mutex<Option<mpsc::Sender<ActionMessage>>>,
    cancel_token: Option<CancellationToken>,
    event_bus: Option<EventBus>,
    db: Option<DatabaseConnection>,
    tick_interval_sec: u64,
    task_timeout_sec: u64,
    max_tasks_per_user: u32,
    max_subscriptions_per_user: u32,
    max_concurrent_actions: u32,
    conditions: Arc<ConditionRegistry>,
    actions: Arc<ActionRegistry>,
}

impl AutomationScheduler {
    pub fn with_registries(
        conditions: Arc<ConditionRegistry>,
        actions: Arc<ActionRegistry>,
    ) -> Self {
        Self {
            background: OwnedBackground::default(),
            initialized: false,
            tasks: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            action_tx: Mutex::new(None),
            cancel_token: None,
            event_bus: None,
            db: None,
            tick_interval_sec: 1,
            task_timeout_sec: 30,
            max_tasks_per_user: 100,
            max_subscriptions_per_user: 50,
            max_concurrent_actions: 5,
            conditions,
            actions,
        }
    }

    pub fn with_runtime(
        db: DatabaseConnection,
        event_bus: EventBus,
        cancel_token: CancellationToken,
        config: SchedulerSection,
        conditions: Arc<ConditionRegistry>,
        actions: Arc<ActionRegistry>,
    ) -> Self {
        Self {
            db: Some(db),
            event_bus: Some(event_bus),
            cancel_token: Some(cancel_token),
            tick_interval_sec: config.tick_interval_sec.max(1),
            task_timeout_sec: config.task_timeout_sec.max(1),
            max_tasks_per_user: config.max_tasks_per_user,
            max_subscriptions_per_user: config.max_subscriptions_per_user,
            max_concurrent_actions: config.max_concurrent_tasks.max(1),
            ..Self::with_registries(conditions, actions)
        }
    }

    async fn tick_loop(
        tasks: Arc<RwLock<HashMap<Uuid, TaskState>>>,
        action_tx: mpsc::Sender<ActionMessage>,
        cancel_token: CancellationToken,
        tick_interval_sec: u64,
        conditions: Arc<ConditionRegistry>,
        db: DatabaseConnection,
    ) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(tick_interval_sec));

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("scheduler tick loop cancelled");
                    break;
                }
                _ = interval.tick() => {
                    Self::check_due_tasks(&tasks, &action_tx, &conditions, &db).await;
                }
            }
        }
    }

    async fn check_due_tasks(
        tasks: &Arc<RwLock<HashMap<Uuid, TaskState>>>,
        action_tx: &mpsc::Sender<ActionMessage>,
        conditions: &ConditionRegistry,
        db: &DatabaseConnection,
    ) {
        let now = Utc::now();
        let candidates = {
            let tasks = tasks.read().await;
            tasks
                .iter()
                .filter_map(|(id, state)| {
                    if !state.task.enabled || state.next_check.is_none_or(|next| next > now) {
                        return None;
                    }
                    if let Some(last) = state.last_triggered {
                        let cooldown = chrono::Duration::seconds(state.task.cooldown_sec as i64);
                        if now - last < cooldown {
                            return None;
                        }
                    }
                    Some((*id, state.task.user_id, state.task.conditions.clone()))
                })
                .collect::<Vec<_>>()
        };

        let source_epochs =
            capture_action_sources(db, candidates.iter().map(|(_, owner, _)| *owner)).await;
        for (task_id, owner, task_conditions) in candidates {
            let Some(source_privacy_epoch) = source_epochs.get(&owner).copied() else {
                continue;
            };
            let matched = conditions
                .evaluate_all(&task_conditions, &ConditionContext::new(None))
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!(%task_id, error = %error, "task condition evaluation failed");
                    false
                });

            if matched
                && action_tx
                    .send(ActionMessage::Task {
                        task_id,
                        request_id: Uuid::new_v4(),
                        source_received_at: now,
                        source_privacy_epoch,
                        source_user_id: owner,
                    })
                    .await
                    .is_err()
            {
                tracing::error!(%task_id, "failed to enqueue task action");
            }

            let mut tasks = tasks.write().await;
            if let Some(state) = tasks.get_mut(&task_id) {
                state.last_checked = Some(now);
                state.next_check =
                    compute_next_check(&state.task.schedule, &state.cron_schedule, Some(now))
                        .unwrap_or_else(|error| {
                            tracing::warn!(
                                %task_id,
                                error = %error,
                                "failed to compute next task check"
                            );
                            None
                        });
            }
        }
    }

    async fn event_loop(
        subscriptions: Arc<RwLock<HashMap<Uuid, SubscriptionState>>>,
        action_tx: mpsc::Sender<ActionMessage>,
        event_bus: EventBus,
        cancel_token: CancellationToken,
        conditions: Arc<ConditionRegistry>,
        db: DatabaseConnection,
    ) {
        let mut rx = event_bus.subscribe();

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("scheduler event loop cancelled");
                    break;
                }
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            Self::handle_event(&subscriptions, &action_tx, &conditions, &db, event).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(lagged = n, "scheduler event loop lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::info!("event bus closed, stopping scheduler event loop");
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn handle_event(
        subscriptions: &Arc<RwLock<HashMap<Uuid, SubscriptionState>>>,
        action_tx: &mpsc::Sender<ActionMessage>,
        conditions: &ConditionRegistry,
        db: &DatabaseConnection,
        event: ServerEvent,
    ) {
        let event_json = serde_json::to_value(&event).unwrap_or_default();
        let now = Utc::now();
        let candidates = {
            let subs = subscriptions.read().await;
            subs.iter()
                .filter_map(|(id, state)| {
                    if !state.sub.enabled || !matches_pattern(&event_json, &state.sub.event_pattern)
                    {
                        return None;
                    }
                    if let Some(last) = state.last_triggered {
                        let cooldown = chrono::Duration::seconds(state.sub.cooldown_sec as i64);
                        if now - last < cooldown {
                            return None;
                        }
                    }
                    Some((
                        *id,
                        state.sub.user_id,
                        state.sub.conditions.clone(),
                        event_json.clone(),
                    ))
                })
                .collect::<Vec<_>>()
        };

        let source_epochs =
            capture_action_sources(db, candidates.iter().map(|(_, owner, _, _)| *owner)).await;
        for (sub_id, owner, sub_conditions, event_data) in candidates {
            let Some(source_privacy_epoch) = source_epochs.get(&owner).copied() else {
                continue;
            };
            let matched = conditions
                .evaluate_all(&sub_conditions, &ConditionContext::new(Some(event_data.clone())))
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!(%sub_id, error = %error, "subscription condition evaluation failed");
                    false
                });
            if matched
                && action_tx
                    .send(ActionMessage::Subscription {
                        subscription_id: sub_id,
                        request_id: Uuid::new_v4(),
                        source_received_at: now,
                        source_privacy_epoch,
                        source_user_id: owner,
                        event: event_data,
                    })
                    .await
                    .is_err()
            {
                tracing::error!(%sub_id, "failed to enqueue subscription action");
            }
        }
    }

    async fn action_loop(
        tasks: Arc<RwLock<HashMap<Uuid, TaskState>>>,
        subscriptions: Arc<RwLock<HashMap<Uuid, SubscriptionState>>>,
        mut action_rx: mpsc::Receiver<ActionMessage>,
        event_bus: EventBus,
        cancel_token: CancellationToken,
        actions: Arc<ActionRegistry>,
        semaphore: Arc<Semaphore>,
        task_timeout_sec: u64,
    ) {
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("scheduler action loop cancelled");
                    break;
                }
                Some(msg) = action_rx.recv() => {
                    match msg {
                        ActionMessage::Task { task_id, request_id, source_received_at, source_privacy_epoch, source_user_id } => {
                            Self::execute_task(
                                &tasks,
                                &event_bus,
                                &actions,
                                semaphore.clone(),
                                task_timeout_sec,
                                task_id,
                                request_id,
                                source_received_at,
                                source_privacy_epoch,
                                source_user_id,
                            ).await;
                        }
                        ActionMessage::Subscription { subscription_id, request_id, source_received_at, source_privacy_epoch, source_user_id, event } => {
                            Self::execute_subscription(
                                &subscriptions,
                                &actions,
                                semaphore.clone(),
                                task_timeout_sec,
                                subscription_id,
                                request_id,
                                source_received_at,
                                source_privacy_epoch,
                                source_user_id,
                                event,
                            ).await;
                        }
                    }
                }
            }
        }
    }

    async fn execute_task(
        tasks: &Arc<RwLock<HashMap<Uuid, TaskState>>>,
        event_bus: &EventBus,
        actions: &ActionRegistry,
        semaphore: Arc<Semaphore>,
        task_timeout_sec: u64,
        task_id: Uuid,
        request_id: Uuid,
        source_received_at: DateTime<Utc>,
        source_privacy_epoch: Option<i64>,
        source_user_id: Uuid,
    ) {
        let task = {
            let tasks = tasks.read().await;
            tasks.get(&task_id).map(|s| s.task.clone())
        };
        let Some(task) = task else {
            tracing::warn!(%task_id, "task not found during execution");
            return;
        };

        if task.user_id != source_user_id {
            tracing::warn!(%task_id, "queued task owner changed; source admission is not transferable");
            return;
        }

        event_bus.publish(ServerEvent::TaskTriggered {
            task_id,
            user_id: task.user_id,
            task_name: task.name.clone(),
            trigger_source: task.schedule.schedule_type.clone(),
        });

        let start = std::time::Instant::now();
        let result = match semaphore.acquire_owned().await {
            Ok(_permit) => tokio::time::timeout(
                std::time::Duration::from_secs(task_timeout_sec.max(1)),
                actions.execute(
                    &task.action,
                    &ActionContext {
                        user_id: task.user_id,
                        request_id,
                        source_received_at,
                        source_privacy_epoch,
                        source: "scheduler".to_string(),
                        trigger_id: Some(task_id),
                        trigger_name: Some(task.name.clone()),
                        event: None,
                    },
                ),
            )
            .await
            .map_err(|_| anyhow::anyhow!("task action timed out"))
            .and_then(|result| result),
            Err(_) => Err(anyhow::anyhow!("scheduler action semaphore closed")),
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(output) => {
                let mut tasks = tasks.write().await;
                if let Some(state) = tasks.get_mut(&task_id) {
                    state.last_triggered = Some(Utc::now());
                    state.retry_count = 0;
                }
                let execution_status = output
                    .pointer("/output/execution_status")
                    .and_then(serde_json::Value::as_str);
                match execution_status {
                    Some("proposed" | "approved" | "dispatched" | "unknown") => {
                        // Acceptance is not external completion. Execution's
                        // ledger owns pending/unknown outcomes and recovery.
                        tracing::info!(%task_id, %request_id, ?execution_status, "scheduled effect remains queryable in execution ledger");
                    }
                    _ => event_bus.publish(ServerEvent::TaskCompleted {
                        task_id,
                        task_name: task.name,
                        success: execution_status.map_or_else(
                            || {
                                output
                                    .get("success")
                                    .and_then(serde_json::Value::as_bool)
                                    .unwrap_or(true)
                            },
                            |status| status == "succeeded",
                        ),
                        duration_ms,
                    }),
                }
            }
            Err(error) => {
                let retry_count = {
                    let mut tasks = tasks.write().await;
                    if let Some(state) = tasks.get_mut(&task_id) {
                        state.retry_count += 1;
                        state.retry_count
                    } else {
                        0
                    }
                };
                event_bus.publish(ServerEvent::TaskFailed {
                    task_id,
                    task_name: task.name,
                    error: error.to_string(),
                    retry_count,
                });
            }
        }
    }

    async fn execute_subscription(
        subscriptions: &Arc<RwLock<HashMap<Uuid, SubscriptionState>>>,
        actions: &ActionRegistry,
        semaphore: Arc<Semaphore>,
        task_timeout_sec: u64,
        sub_id: Uuid,
        request_id: Uuid,
        source_received_at: DateTime<Utc>,
        source_privacy_epoch: Option<i64>,
        source_user_id: Uuid,
        event_data: serde_json::Value,
    ) {
        let sub = {
            let subs = subscriptions.read().await;
            subs.get(&sub_id).map(|s| s.sub.clone())
        };
        let Some(sub) = sub else {
            tracing::warn!(%sub_id, "subscription not found during execution");
            return;
        };

        if sub.user_id != source_user_id {
            tracing::warn!(%sub_id, "queued subscription owner changed; source admission is not transferable");
            return;
        }
        let result = match semaphore.acquire_owned().await {
            Ok(_permit) => tokio::time::timeout(
                std::time::Duration::from_secs(task_timeout_sec.max(1)),
                actions.execute(
                    &sub.action,
                    &ActionContext {
                        user_id: sub.user_id,
                        request_id,
                        source_received_at,
                        source_privacy_epoch,
                        source: "subscription".to_string(),
                        trigger_id: Some(sub_id),
                        trigger_name: Some(sub.name.clone()),
                        event: Some(event_data),
                    },
                ),
            )
            .await
            .map_err(|_| anyhow::anyhow!("subscription action timed out"))
            .and_then(|result| result),
            Err(_) => Err(anyhow::anyhow!("scheduler action semaphore closed")),
        };

        if let Err(error) = result {
            tracing::warn!(%sub_id, error = %error, "subscription action failed");
        }

        let mut subs = subscriptions.write().await;
        if let Some(state) = subs.get_mut(&sub_id) {
            state.last_triggered = Some(Utc::now());
        }
    }
}

impl Default for AutomationScheduler {
    fn default() -> Self {
        Self::with_registries(
            Arc::new(ConditionRegistry::new()),
            Arc::new(ActionRegistry::new()),
        )
    }
}

#[async_trait]
impl ModuleLifecycle for AutomationScheduler {
    fn name(&self) -> &'static str {
        "automation_scheduler"
    }

    async fn init(&mut self) -> anyhow::Result<()> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("scheduler database not configured"))?;
        let (tasks, subscriptions) = load_scheduler_state_from_db(db).await?;
        *self.tasks.write().await = tasks;
        *self.subscriptions.write().await = subscriptions;
        self.initialized = true;
        tracing::info!("AutomationScheduler initialized");
        Ok(())
    }

    async fn start(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.initialized, "scheduler not initialized");
        let parent = self
            .cancel_token
            .clone()
            .ok_or_else(|| anyhow::anyhow!("scheduler not initialized"))?;
        let mut background = self.background.begin_start(&parent).await?;
        let cancel_token = background.token();
        let (action_tx, action_rx) = mpsc::channel::<ActionMessage>(256);
        *self.action_tx.lock().await = Some(action_tx.clone());

        let event_bus = self
            .event_bus
            .clone()
            .ok_or_else(|| anyhow::anyhow!("scheduler not initialized"))?;
        let db = self
            .db
            .clone()
            .ok_or_else(|| anyhow::anyhow!("scheduler database not initialized"))?;
        let tick_interval_sec = self.tick_interval_sec;
        let task_timeout_sec = self.task_timeout_sec;
        let max_concurrent_actions = self.max_concurrent_actions;
        let semaphore = Arc::new(Semaphore::new(max_concurrent_actions as usize));

        background.spawn_loop(
            "scheduler ticks",
            Self::tick_loop(
                self.tasks.clone(),
                action_tx.clone(),
                cancel_token.clone(),
                tick_interval_sec,
                self.conditions.clone(),
                db.clone(),
            ),
        );
        background.spawn_loop(
            "scheduler events",
            Self::event_loop(
                self.subscriptions.clone(),
                action_tx,
                event_bus.clone(),
                cancel_token.clone(),
                self.conditions.clone(),
                db,
            ),
        );
        background.spawn_loop(
            "scheduler actions",
            Self::action_loop(
                self.tasks.clone(),
                self.subscriptions.clone(),
                action_rx,
                event_bus,
                cancel_token,
                self.actions.clone(),
                semaphore,
                task_timeout_sec,
            ),
        );

        background.commit();
        tracing::info!("AutomationScheduler started");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.background.request_stop();
        self.action_tx.lock().await.take();
        self.background.stop().await
    }

    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }
}

#[async_trait]
impl SchedulerBackend for AutomationScheduler {
    async fn create_task(&self, task: ScheduledTask) -> anyhow::Result<Uuid> {
        let id = task.id;
        let max_tasks_per_user = self.max_tasks_per_user;
        if max_tasks_per_user > 0 {
            let tasks = self.tasks.read().await;
            let count = tasks
                .values()
                .filter(|state| state.task.user_id == task.user_id)
                .count();
            if count >= max_tasks_per_user as usize {
                anyhow::bail!("max tasks per user exceeded");
            }
        }
        if let Some(db) = &self.db {
            upsert_task(db, &task).await?;
        }
        self.tasks.write().await.insert(id, TaskState::new(task)?);
        Ok(id)
    }

    async fn update_task(&self, task_id: Uuid, patch: TaskPatch) -> anyhow::Result<()> {
        let mut tasks = self.tasks.write().await;
        let state = tasks
            .get_mut(&task_id)
            .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))?;
        if let Some(name) = patch.name {
            state.task.name = name;
        }
        if let Some(enabled) = patch.enabled {
            state.task.enabled = enabled;
        }
        if let Some(schedule) = patch.schedule {
            state.task.schedule = schedule;
            state.refresh_schedule()?;
        }
        if let Some(conditions) = patch.conditions {
            state.task.conditions = conditions;
        }
        if let Some(action) = patch.action {
            state.task.action = action;
        }
        let task = state.task.clone();
        drop(tasks);
        if let Some(db) = &self.db {
            upsert_task(db, &task).await?;
        }
        Ok(())
    }

    async fn delete_task(&self, task_id: Uuid) -> anyhow::Result<()> {
        // Idempotent cleanup also covers create committed with a lost ACK.
        // Stop local dispatch immediately; failed durable deletion stays an
        // error and can be retried even after this local entry is removed.
        self.tasks.write().await.remove(&task_id);
        if let Some(db) = &self.db {
            db.execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "DELETE FROM scheduled_tasks WHERE id = $1::uuid",
                vec![task_id.to_string().into()],
            ))
            .await?;
        }
        Ok(())
    }

    async fn list_tasks(&self, filter: TaskFilter) -> anyhow::Result<Vec<ScheduledTask>> {
        let tasks = self.tasks.read().await;
        Ok(tasks
            .values()
            .filter(|state| {
                if let Some(user_id) = filter.user_id
                    && state.task.user_id != user_id
                {
                    return false;
                }
                if filter.enabled_only && !state.task.enabled {
                    return false;
                }
                true
            })
            .map(|state| state.task.clone())
            .collect())
    }

    async fn pause_task(&self, task_id: Uuid) -> anyhow::Result<()> {
        self.update_task(
            task_id,
            TaskPatch {
                name: None,
                enabled: Some(false),
                schedule: None,
                conditions: None,
                action: None,
            },
        )
        .await
    }

    async fn resume_task(&self, task_id: Uuid) -> anyhow::Result<()> {
        self.update_task(
            task_id,
            TaskPatch {
                name: None,
                enabled: Some(true),
                schedule: None,
                conditions: None,
                action: None,
            },
        )
        .await
    }

    async fn subscribe_event(&self, sub: EventSubscription) -> anyhow::Result<Uuid> {
        let id = sub.id;
        let max_subscriptions_per_user = self.max_subscriptions_per_user;
        if max_subscriptions_per_user > 0 {
            let subs = self.subscriptions.read().await;
            let count = subs
                .values()
                .filter(|state| state.sub.user_id == sub.user_id)
                .count();
            if count >= max_subscriptions_per_user as usize {
                anyhow::bail!("max subscriptions per user exceeded");
            }
        }
        if let Some(db) = &self.db {
            upsert_subscription(db, &sub).await?;
        }
        self.subscriptions.write().await.insert(
            id,
            SubscriptionState {
                sub,
                last_triggered: None,
            },
        );
        Ok(id)
    }

    async fn unsubscribe_event(&self, sub_id: Uuid) -> anyhow::Result<()> {
        self.subscriptions
            .write()
            .await
            .remove(&sub_id)
            .ok_or_else(|| anyhow::anyhow!("subscription not found: {sub_id}"))?;
        if let Some(db) = &self.db {
            db.execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "DELETE FROM event_subscriptions WHERE id = $1::uuid",
                vec![sub_id.to_string().into()],
            ))
            .await?;
        }
        Ok(())
    }

    async fn list_subscriptions(&self, user_id: Uuid) -> anyhow::Result<Vec<EventSubscription>> {
        let subs = self.subscriptions.read().await;
        Ok(subs
            .values()
            .filter(|state| state.sub.user_id == user_id)
            .map(|state| state.sub.clone())
            .collect())
    }
}

fn parse_cron_schedule(schedule: &ScheduleConfig) -> anyhow::Result<Option<Schedule>> {
    if schedule.schedule_type != "cron" {
        return Ok(None);
    }
    let expr = schedule
        .cron_expression
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("cron schedule requires cron_expression"))?;
    let normalized_expr = normalize_cron_expression(expr)?;
    Ok(Some(Schedule::from_str(&normalized_expr).map_err(|e| {
        anyhow::anyhow!("invalid cron expression: {e}")
    })?))
}

fn normalize_cron_expression(expr: &str) -> anyhow::Result<String> {
    let trimmed = expr.trim();
    let field_count = trimmed.split_whitespace().count();
    match field_count {
        // Accept standard Unix 5-field cron and make seconds explicit for the
        // `cron` crate, which expects seconds as the first field.
        5 => Ok(format!("0 {trimmed}")),
        6 | 7 => Ok(trimmed.to_string()),
        _ => anyhow::bail!(
            "cron expression must have 5, 6, or 7 fields; got {field_count}: {trimmed}"
        ),
    }
}

fn compute_next_check(
    schedule: &ScheduleConfig,
    cron_schedule: &Option<Schedule>,
    after: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<DateTime<Utc>>> {
    let base = after.unwrap_or_else(Utc::now);
    match schedule.schedule_type.as_str() {
        "cron" => {
            let schedule = cron_schedule
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("cron schedule not initialized"))?;
            Ok(schedule.after(&base).next())
        }
        "once" => {
            let run_at = schedule
                .run_at
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("once schedule requires run_at"))?;
            let dt = DateTime::parse_from_rfc3339(run_at)
                .map_err(|e| anyhow::anyhow!("invalid run_at: {e}"))?
                .with_timezone(&Utc);
            Ok((dt > base).then_some(dt))
        }
        "interval" => {
            let every = schedule
                .every_sec
                .ok_or_else(|| anyhow::anyhow!("interval schedule requires every_sec"))?;
            Ok(Some(base + chrono::Duration::seconds(every as i64)))
        }
        "condition" => {
            let every = schedule.poll_interval_sec.unwrap_or(5).max(1);
            Ok(Some(base + chrono::Duration::seconds(every as i64)))
        }
        other => anyhow::bail!("unknown schedule type: {other}"),
    }
}

pub fn matches_pattern(event: &serde_json::Value, pattern: &serde_json::Value) -> bool {
    match (event, pattern) {
        (_, serde_json::Value::Null) => true,
        (serde_json::Value::Object(ev), serde_json::Value::Object(pat)) => pat
            .iter()
            .all(|(k, v)| ev.get(k).is_some_and(|ev_v| matches_pattern(ev_v, v))),
        (ev, pat) => ev == pat,
    }
}

async fn load_scheduler_state_from_db(
    db: &DatabaseConnection,
) -> anyhow::Result<(HashMap<Uuid, TaskState>, HashMap<Uuid, SubscriptionState>)> {
    let mut tasks = HashMap::new();
    let task_rows = db
        .query_all(Statement::from_string(
            DatabaseBackend::Postgres,
            r#"
            SELECT id::text, user_id::text, name, enabled, schedule, conditions,
                   action, cooldown_sec, max_retries, created_by
            FROM scheduled_tasks
            "#,
        ))
        .await?;
    for row in task_rows {
        let id = Uuid::parse_str(&row.try_get::<String>("", "id")?)?;
        let user_id = Uuid::parse_str(&row.try_get::<String>("", "user_id")?)?;
        let cooldown_sec: i32 = row.try_get("", "cooldown_sec")?;
        let max_retries: i32 = row.try_get("", "max_retries")?;
        let schedule_value: serde_json::Value = row.try_get("", "schedule")?;
        let task = ScheduledTask {
            id,
            user_id,
            name: row.try_get("", "name")?,
            enabled: row.try_get("", "enabled")?,
            schedule: serde_json::from_value(schedule_value)?,
            conditions: row.try_get("", "conditions")?,
            action: row.try_get("", "action")?,
            cooldown_sec: cooldown_sec.max(0) as u32,
            max_retries: max_retries.max(0) as u32,
            created_by: row.try_get("", "created_by")?,
        };
        tasks.insert(id, TaskState::new(task)?);
    }

    let mut subscriptions = HashMap::new();
    let sub_rows = db
        .query_all(Statement::from_string(
            DatabaseBackend::Postgres,
            r#"
            SELECT id::text, user_id::text, name, enabled, event_pattern,
                   conditions, action, cooldown_sec, created_by
            FROM event_subscriptions
            "#,
        ))
        .await?;
    for row in sub_rows {
        let id = Uuid::parse_str(&row.try_get::<String>("", "id")?)?;
        let user_id = Uuid::parse_str(&row.try_get::<String>("", "user_id")?)?;
        let cooldown_sec: i32 = row.try_get("", "cooldown_sec")?;
        subscriptions.insert(
            id,
            SubscriptionState {
                sub: EventSubscription {
                    id,
                    user_id,
                    name: row.try_get("", "name")?,
                    enabled: row.try_get("", "enabled")?,
                    event_pattern: row.try_get("", "event_pattern")?,
                    conditions: row.try_get("", "conditions")?,
                    action: row.try_get("", "action")?,
                    cooldown_sec: cooldown_sec.max(0) as u32,
                    created_by: row.try_get("", "created_by")?,
                },
                last_triggered: None,
            },
        );
    }

    Ok((tasks, subscriptions))
}

async fn upsert_task(db: &DatabaseConnection, task: &ScheduledTask) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"
        INSERT INTO scheduled_tasks
            (id, user_id, name, enabled, schedule, conditions, action,
             cooldown_sec, max_retries, created_by, updated_at)
        VALUES
            ($1::uuid, $2::uuid, $3, $4, $5::jsonb, $6::jsonb, $7::jsonb,
             $8, $9, $10, NOW())
        ON CONFLICT (id) DO UPDATE SET
            user_id = EXCLUDED.user_id,
            name = EXCLUDED.name,
            enabled = EXCLUDED.enabled,
            schedule = EXCLUDED.schedule,
            conditions = EXCLUDED.conditions,
            action = EXCLUDED.action,
            cooldown_sec = EXCLUDED.cooldown_sec,
            max_retries = EXCLUDED.max_retries,
            created_by = EXCLUDED.created_by,
            updated_at = NOW()
        "#,
        vec![
            task.id.to_string().into(),
            task.user_id.to_string().into(),
            task.name.clone().into(),
            task.enabled.into(),
            serde_json::to_value(&task.schedule)?.into(),
            task.conditions.clone().into(),
            task.action.clone().into(),
            (task.cooldown_sec as i32).into(),
            (task.max_retries as i32).into(),
            task.created_by.clone().into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn upsert_subscription(
    db: &DatabaseConnection,
    sub: &EventSubscription,
) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"
        INSERT INTO event_subscriptions
            (id, user_id, name, enabled, event_pattern, conditions, action,
             cooldown_sec, created_by, updated_at)
        VALUES
            ($1::uuid, $2::uuid, $3, $4, $5::jsonb, $6::jsonb, $7::jsonb,
             $8, $9, NOW())
        ON CONFLICT (id) DO UPDATE SET
            user_id = EXCLUDED.user_id,
            name = EXCLUDED.name,
            enabled = EXCLUDED.enabled,
            event_pattern = EXCLUDED.event_pattern,
            conditions = EXCLUDED.conditions,
            action = EXCLUDED.action,
            cooldown_sec = EXCLUDED.cooldown_sec,
            created_by = EXCLUDED.created_by,
            updated_at = NOW()
        "#,
        vec![
            sub.id.to_string().into(),
            sub.user_id.to_string().into(),
            sub.name.clone().into(),
            sub.enabled.into(),
            sub.event_pattern.clone().into(),
            sub.conditions.clone().into(),
            sub.action.clone().into(),
            (sub.cooldown_sec as i32).into(),
            sub.created_by.clone().into(),
        ],
    ))
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cron_config(expr: &str) -> ScheduleConfig {
        ScheduleConfig {
            schedule_type: "cron".to_string(),
            cron_expression: Some(expr.to_string()),
            run_at: None,
            every_sec: None,
            poll_interval_sec: None,
        }
    }

    #[test]
    fn parses_standard_five_field_cron_by_adding_seconds() {
        let schedule = parse_cron_schedule(&cron_config("0 3 * * *"))
            .unwrap()
            .expect("cron schedule");
        assert!(schedule.upcoming(Utc).next().is_some());
    }

    #[test]
    fn parses_explicit_seconds_cron() {
        let schedule = parse_cron_schedule(&cron_config("0 0 3 * * *"))
            .unwrap()
            .expect("cron schedule");
        assert!(schedule.upcoming(Utc).next().is_some());
    }

    #[test]
    fn rejects_malformed_cron_field_counts() {
        let error = parse_cron_schedule(&cron_config("0 3 * *"))
            .expect_err("invalid cron should fail")
            .to_string();
        assert!(error.contains("5, 6, or 7 fields"));
    }
}
