//! CronScheduler: Full scheduler implementation with cron, one-time, and interval scheduling.
//!
//! ## Architecture
//! - Tasks and subscriptions stored in `RwLock<HashMap>` (in-memory, load from DB on init)
//! - Background tick loop fires due tasks every `tick_interval_sec`
//! - Event listener processes EventBus events against subscriptions
//!
//! ## Schedule Types
//! - `cron`: Cron expression (e.g., "0 9 * * *" for daily at 9am)
//! - `once`: Single execution at specified time (ISO 8601)
//! - `interval`: Repeated execution every N seconds

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cron::Schedule;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::event::{EventBus, ServerEvent};
use crate::module::lifecycle::{ModuleContext, ModuleLifecycle};
use crate::providers::adapter::{
    EventSubscription, ScheduleConfig, ScheduledTask, SchedulerBackend, TaskFilter, TaskPatch,
};
use super::condition::ConditionRegistry;

/// Internal task state tracking next run time and retry count
#[derive(Debug, Clone)]
struct TaskState {
    task: ScheduledTask,
    next_run: Option<DateTime<Utc>>,
    last_run: Option<DateTime<Utc>>,
    retry_count: u32,
    cron_schedule: Option<Schedule>,
}

impl TaskState {
    fn new(task: ScheduledTask) -> anyhow::Result<Self> {
        let cron_schedule = if task.schedule.schedule_type == "cron" {
            let expr = task
                .schedule
                .cron_expression
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("cron type requires cron_expression"))?;
            Some(
                Schedule::from_str(expr)
                    .map_err(|e| anyhow::anyhow!("invalid cron expression: {e}"))?,
            )
        } else {
            None
        };

        let next_run = Self::compute_next_run(&task.schedule, &cron_schedule, None)?;

        Ok(Self {
            task,
            next_run,
            last_run: None,
            retry_count: 0,
            cron_schedule,
        })
    }

    fn compute_next_run(
        schedule: &ScheduleConfig,
        cron_schedule: &Option<Schedule>,
        after: Option<DateTime<Utc>>,
    ) -> anyhow::Result<Option<DateTime<Utc>>> {
        let base = after.unwrap_or_else(Utc::now);

        match schedule.schedule_type.as_str() {
            "cron" => {
                let sched = cron_schedule
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("cron schedule not initialized"))?;
                Ok(sched.after(&base).next())
            }
            "once" => {
                let run_at = schedule
                    .run_at
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("once type requires run_at"))?;
                let dt = DateTime::parse_from_rfc3339(run_at)
                    .map_err(|e| anyhow::anyhow!("invalid run_at format: {e}"))?
                    .with_timezone(&Utc);
                if dt > base {
                    Ok(Some(dt))
                } else {
                    Ok(None) // Already passed
                }
            }
            "interval" => {
                let every = schedule
                    .every_sec
                    .ok_or_else(|| anyhow::anyhow!("interval type requires every_sec"))?;
                Ok(Some(base + chrono::Duration::seconds(every as i64)))
            }
            _ => anyhow::bail!("unknown schedule type: {}", schedule.schedule_type),
        }
    }

    fn advance(&mut self) -> anyhow::Result<()> {
        self.last_run = Some(Utc::now());
        self.retry_count = 0;
        self.next_run =
            Self::compute_next_run(&self.task.schedule, &self.cron_schedule, self.last_run)?;
        Ok(())
    }
}

/// Subscription state tracking last trigger time for cooldown
#[derive(Debug, Clone)]
struct SubscriptionState {
    sub: EventSubscription,
    last_triggered: Option<DateTime<Utc>>,
}

/// Message type for internal action queue
enum ActionMessage {
    Task(Uuid),
    Subscription(Uuid, serde_json::Value),
}

/// CronScheduler: Full implementation of SchedulerBackend
pub struct CronScheduler {
    tasks: Arc<RwLock<HashMap<Uuid, TaskState>>>,
    subscriptions: Arc<RwLock<HashMap<Uuid, SubscriptionState>>>,
    action_tx: Mutex<Option<mpsc::Sender<ActionMessage>>>,
    cancel_token: Mutex<Option<CancellationToken>>,
    event_bus: Mutex<Option<EventBus>>,
    conditions: Arc<ConditionRegistry>,
}

impl CronScheduler {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            action_tx: Mutex::new(None),
            cancel_token: Mutex::new(None),
            event_bus: Mutex::new(None),
            conditions: Arc::new(ConditionRegistry::new()),
        }
    }

    /// Create a scheduler with a pre-configured condition registry.
    pub fn with_conditions(conditions: Arc<ConditionRegistry>) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            action_tx: Mutex::new(None),
            cancel_token: Mutex::new(None),
            event_bus: Mutex::new(None),
            conditions,
        }
    }

    /// Get a reference to the condition registry (for plugin registration).
    pub fn condition_registry(&self) -> &Arc<ConditionRegistry> {
        &self.conditions
    }

    /// Background tick loop: checks for due tasks
    async fn tick_loop(
        tasks: Arc<RwLock<HashMap<Uuid, TaskState>>>,
        action_tx: mpsc::Sender<ActionMessage>,
        cancel_token: CancellationToken,
        tick_interval_sec: u64,
        conditions: Arc<ConditionRegistry>,
    ) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(tick_interval_sec));

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("scheduler tick loop cancelled");
                    break;
                }
                _ = interval.tick() => {
                    let now = Utc::now();
                    // Collect time-due candidates
                    let candidates: Vec<(Uuid, serde_json::Value)> = {
                        let tasks = tasks.read().unwrap();
                        tasks.iter()
                            .filter(|(_, state)| {
                                state.task.enabled
                                    && state.next_run.map_or(false, |nr| nr <= now)
                            })
                            .map(|(id, state)| (*id, state.task.conditions.clone()))
                            .collect()
                    };

                    for (task_id, task_conditions) in candidates {
                        if conditions.evaluate_all(&task_conditions).await {
                            if action_tx.send(ActionMessage::Task(task_id)).await.is_err() {
                                tracing::error!("failed to send task action message");
                            }
                        }
                    }
                }
            }
        }
    }

    /// Event listener loop: matches events against subscriptions
    async fn event_loop(
        subscriptions: Arc<RwLock<HashMap<Uuid, SubscriptionState>>>,
        action_tx: mpsc::Sender<ActionMessage>,
        event_bus: EventBus,
        cancel_token: CancellationToken,
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
                            Self::handle_event(&subscriptions, &action_tx, event).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(lagged = n, "scheduler event loop lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::info!("event bus closed, stopping event loop");
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
        event: ServerEvent,
    ) {
        let event_json = serde_json::to_value(&event).unwrap_or_default();
        let now = Utc::now();

        let matching_subs: Vec<(Uuid, serde_json::Value)> = {
            let subs = subscriptions.read().unwrap();
            subs.iter()
                .filter(|(_, state)| {
                    if !state.sub.enabled {
                        return false;
                    }
                    // Check cooldown
                    if let Some(last) = state.last_triggered {
                        let cooldown = chrono::Duration::seconds(state.sub.cooldown_sec as i64);
                        if now - last < cooldown {
                            return false;
                        }
                    }
                    // Match event pattern
                    Self::matches_pattern(&event_json, &state.sub.event_pattern)
                })
                .map(|(id, _)| (*id, event_json.clone()))
                .collect()
        };

        for (sub_id, event_data) in matching_subs {
            if action_tx
                .send(ActionMessage::Subscription(sub_id, event_data))
                .await
                .is_err()
            {
                tracing::error!("failed to send subscription action message");
            }
        }
    }

    /// Simple pattern matching: checks if event contains all fields in pattern
    pub fn matches_pattern(event: &serde_json::Value, pattern: &serde_json::Value) -> bool {
        match (event, pattern) {
            (serde_json::Value::Object(ev), serde_json::Value::Object(pat)) => {
                pat.iter().all(|(k, v)| {
                    ev.get(k).map_or(false, |ev_v| Self::matches_pattern(ev_v, v))
                })
            }
            (ev, pat) => ev == pat,
        }
    }

    /// Action executor loop: runs task/subscription actions
    async fn action_loop(
        tasks: Arc<RwLock<HashMap<Uuid, TaskState>>>,
        subscriptions: Arc<RwLock<HashMap<Uuid, SubscriptionState>>>,
        mut action_rx: mpsc::Receiver<ActionMessage>,
        event_bus: EventBus,
        cancel_token: CancellationToken,
    ) {
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("scheduler action loop cancelled");
                    break;
                }
                Some(msg) = action_rx.recv() => {
                    match msg {
                        ActionMessage::Task(task_id) => {
                            Self::execute_task(&tasks, &event_bus, task_id).await;
                        }
                        ActionMessage::Subscription(sub_id, event_data) => {
                            Self::execute_subscription(&subscriptions, &event_bus, sub_id, event_data).await;
                        }
                    }
                }
            }
        }
    }

    async fn execute_task(
        tasks: &Arc<RwLock<HashMap<Uuid, TaskState>>>,
        event_bus: &EventBus,
        task_id: Uuid,
    ) {
        let task_info = {
            let tasks = tasks.read().unwrap();
            tasks.get(&task_id).map(|s| (s.task.name.clone(), s.task.action.clone()))
        };

        let Some((task_name, action)) = task_info else {
            tracing::warn!(%task_id, "task not found during execution");
            return;
        };

        // Publish task triggered event
        event_bus.publish(ServerEvent::TaskTriggered {
            task_id,
            task_name: task_name.clone(),
            trigger_source: "scheduler".to_string(),
        });

        let start = std::time::Instant::now();
        let result = Self::run_action(&action).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(_) => {
                // Advance task to next run
                {
                    let mut tasks = tasks.write().unwrap();
                    if let Some(state) = tasks.get_mut(&task_id) {
                        if let Err(e) = state.advance() {
                            tracing::error!(%task_id, error = %e, "failed to advance task");
                        }
                    }
                }
                event_bus.publish(ServerEvent::TaskCompleted {
                    task_id,
                    task_name,
                    success: true,
                    duration_ms,
                });
            }
            Err(e) => {
                let retry_count = {
                    let mut tasks = tasks.write().unwrap();
                    if let Some(state) = tasks.get_mut(&task_id) {
                        state.retry_count += 1;
                        state.retry_count
                    } else {
                        0
                    }
                };
                event_bus.publish(ServerEvent::TaskFailed {
                    task_id,
                    task_name,
                    error: e.to_string(),
                    retry_count,
                });
            }
        }
    }

    async fn execute_subscription(
        subscriptions: &Arc<RwLock<HashMap<Uuid, SubscriptionState>>>,
        _event_bus: &EventBus,
        sub_id: Uuid,
        _event_data: serde_json::Value,
    ) {
        let action = {
            let subs = subscriptions.read().unwrap();
            subs.get(&sub_id).map(|s| s.sub.action.clone())
        };

        let Some(action) = action else {
            tracing::warn!(%sub_id, "subscription not found during execution");
            return;
        };

        let result = Self::run_action(&action).await;
        
        // Update last_triggered
        {
            let mut subs = subscriptions.write().unwrap();
            if let Some(state) = subs.get_mut(&sub_id) {
                state.last_triggered = Some(Utc::now());
            }
        }

        if let Err(e) = result {
            tracing::error!(%sub_id, error = %e, "subscription action failed");
        }
    }

    /// Execute an action config
    /// TODO: Integrate with ToolRegistry for action execution
    async fn run_action(action: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let action_type = action
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        match action_type {
            "log" => {
                let message = action
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("no message");
                tracing::info!(action = "log", %message, "executing action");
                Ok(serde_json::json!({"logged": message}))
            }
            "tool_call" => {
                // TODO: Dispatch to ToolRegistry
                let tool_name = action
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                tracing::info!(action = "tool_call", %tool_name, "would execute tool");
                Ok(serde_json::json!({"tool": tool_name, "status": "not_implemented"}))
            }
            "ai_prompt" => {
                // TODO: Dispatch to LLM
                let prompt = action
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                tracing::info!(action = "ai_prompt", prompt_len = prompt.len(), "would execute AI prompt");
                Ok(serde_json::json!({"prompt_len": prompt.len(), "status": "not_implemented"}))
            }
            _ => {
                tracing::warn!(action_type, "unknown action type");
                anyhow::bail!("unknown action type: {action_type}")
            }
        }
    }
}

impl Default for CronScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for CronScheduler {
    fn name(&self) -> &'static str {
        "cron_scheduler"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        *self.cancel_token.lock().await = Some(ctx.cancel_token.clone());
        *self.event_bus.lock().await = Some(ctx.event_bus.clone());

        // TODO: Load tasks and subscriptions from database
        tracing::info!("CronScheduler initialized");
        Ok(())
    }

    async fn start(&self) -> anyhow::Result<()> {
        let (action_tx, action_rx) = mpsc::channel::<ActionMessage>(256);
        *self.action_tx.lock().await = Some(action_tx.clone());

        let cancel_token = self
            .cancel_token
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("scheduler not initialized"))?;
        let event_bus = self
            .event_bus
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("scheduler not initialized"))?;

        // Start tick loop
        let tasks_clone = self.tasks.clone();
        let action_tx_clone = action_tx.clone();
        let cancel_clone = cancel_token.clone();
        let conditions_clone = self.conditions.clone();
        tokio::spawn(async move {
            Self::tick_loop(tasks_clone, action_tx_clone, cancel_clone, 1, conditions_clone).await;
        });

        // Start event loop
        let subs_clone = self.subscriptions.clone();
        let action_tx_clone = action_tx;
        let event_bus_clone = event_bus.clone();
        let cancel_clone = cancel_token.clone();
        tokio::spawn(async move {
            Self::event_loop(subs_clone, action_tx_clone, event_bus_clone, cancel_clone).await;
        });

        // Start action loop
        let tasks_clone = self.tasks.clone();
        let subs_clone = self.subscriptions.clone();
        tokio::spawn(async move {
            Self::action_loop(tasks_clone, subs_clone, action_rx, event_bus, cancel_token).await;
        });

        tracing::info!("CronScheduler started");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        // Cancel token will stop all loops
        tracing::info!("CronScheduler stopped");
        Ok(())
    }
}

#[async_trait]
impl SchedulerBackend for CronScheduler {
    async fn create_task(&self, task: ScheduledTask) -> anyhow::Result<Uuid> {
        let task_id = task.id;
        let state = TaskState::new(task)?;

        {
            let mut tasks = self.tasks.write().unwrap();
            tasks.insert(task_id, state);
        }

        tracing::info!(%task_id, "task created");
        Ok(task_id)
    }

    async fn update_task(&self, task_id: Uuid, patch: TaskPatch) -> anyhow::Result<()> {
        let mut tasks = self.tasks.write().unwrap();
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
            // Recompute cron schedule and next_run
            if state.task.schedule.schedule_type == "cron" {
                let expr = state
                    .task
                    .schedule
                    .cron_expression
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("cron type requires cron_expression"))?;
                state.cron_schedule = Some(
                    Schedule::from_str(expr)
                        .map_err(|e| anyhow::anyhow!("invalid cron expression: {e}"))?,
                );
            } else {
                state.cron_schedule = None;
            }
            state.next_run = TaskState::compute_next_run(
                &state.task.schedule,
                &state.cron_schedule,
                state.last_run,
            )?;
        }
        if let Some(conditions) = patch.conditions {
            state.task.conditions = conditions;
        }
        if let Some(action) = patch.action {
            state.task.action = action;
        }

        tracing::info!(%task_id, "task updated");
        Ok(())
    }

    async fn delete_task(&self, task_id: Uuid) -> anyhow::Result<()> {
        let mut tasks = self.tasks.write().unwrap();
        tasks
            .remove(&task_id)
            .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))?;
        tracing::info!(%task_id, "task deleted");
        Ok(())
    }

    async fn list_tasks(&self, filter: TaskFilter) -> anyhow::Result<Vec<ScheduledTask>> {
        let tasks = self.tasks.read().unwrap();
        let result: Vec<ScheduledTask> = tasks
            .values()
            .filter(|state| {
                if let Some(user_id) = filter.user_id {
                    if state.task.user_id != user_id {
                        return false;
                    }
                }
                if filter.enabled_only && !state.task.enabled {
                    return false;
                }
                true
            })
            .map(|state| state.task.clone())
            .collect();
        Ok(result)
    }

    async fn pause_task(&self, task_id: Uuid) -> anyhow::Result<()> {
        let mut tasks = self.tasks.write().unwrap();
        let state = tasks
            .get_mut(&task_id)
            .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))?;
        state.task.enabled = false;
        tracing::info!(%task_id, "task paused");
        Ok(())
    }

    async fn resume_task(&self, task_id: Uuid) -> anyhow::Result<()> {
        let mut tasks = self.tasks.write().unwrap();
        let state = tasks
            .get_mut(&task_id)
            .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))?;
        state.task.enabled = true;
        // Recompute next_run from now
        state.next_run = TaskState::compute_next_run(
            &state.task.schedule,
            &state.cron_schedule,
            Some(chrono::Utc::now()),
        )?;
        tracing::info!(%task_id, "task resumed");
        Ok(())
    }

    async fn subscribe_event(&self, sub: EventSubscription) -> anyhow::Result<Uuid> {
        let sub_id = sub.id;
        let state = SubscriptionState {
            sub,
            last_triggered: None,
        };

        {
            let mut subs = self.subscriptions.write().unwrap();
            subs.insert(sub_id, state);
        }

        tracing::info!(%sub_id, "subscription created");
        Ok(sub_id)
    }

    async fn unsubscribe_event(&self, sub_id: Uuid) -> anyhow::Result<()> {
        let mut subs = self.subscriptions.write().unwrap();
        subs.remove(&sub_id)
            .ok_or_else(|| anyhow::anyhow!("subscription not found: {sub_id}"))?;
        tracing::info!(%sub_id, "subscription deleted");
        Ok(())
    }

    async fn list_subscriptions(&self, user_id: Uuid) -> anyhow::Result<Vec<EventSubscription>> {
        let subs = self.subscriptions.read().unwrap();
        let result: Vec<EventSubscription> = subs
            .values()
            .filter(|state| state.sub.user_id == user_id)
            .map(|state| state.sub.clone())
            .collect();
        Ok(result)
    }
}
