//! ScheduledMeditation: Full implementation of MeditationStrategy.
//!
//! Triggers meditation phases via:
//! - Event-driven: ConversationEnded → Phase 1 (extraction)
//! - Event-driven: Idle detection → Phase 2 (consolidation)
//! - Scheduler: reflection_schedule → Phase 3 (reflection) via CronScheduler
//! - Scheduler: habit_detection_schedule → Phase 4 (habit detection) via CronScheduler
//! - Memory threshold: pending episodic memories > threshold → Phase 2
//! - Manual trigger via API

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::MeditationSection;
use crate::entity::meditation_log;
use crate::event::{EventBus, ServerEvent};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    MeditationPhase, MeditationStatus, MeditationStrategy, MeditationTask, ScheduleConfig,
    ScheduledTask, SchedulerBackend,
};
use crate::scheduler::ConditionRegistry;

/// Well-known task name prefix for meditation cron tasks
const TASK_NAME_REFLECTION: &str = "meditation_reflection";
const TASK_NAME_HABIT_DETECTION: &str = "meditation_habit_detection";

/// Internal state tracking last execution and pending tasks
struct MeditationState {
    last_phase: Option<String>,
    last_executed_at: Option<DateTime<Utc>>,
    last_interaction_at: Option<DateTime<Utc>>,
    /// User-level condition overrides; None = use config defaults
    conditions_override: Option<Vec<serde_json::Value>>,
}

pub struct ScheduledMeditation {
    config: Mutex<Option<MeditationSection>>,
    db: Mutex<Option<DatabaseConnection>>,
    event_bus: Mutex<Option<EventBus>>,
    cancel_token: Mutex<Option<CancellationToken>>,
    scheduler: Mutex<Option<Arc<dyn SchedulerBackend>>>,
    condition_registry: Mutex<Option<Arc<ConditionRegistry>>>,
    /// Task IDs registered with the scheduler (for cleanup on stop)
    reflection_task_id: Mutex<Option<Uuid>>,
    habit_task_id: Mutex<Option<Uuid>>,
    enabled: AtomicBool,
    pending_tasks: AtomicU32,
    concurrency_semaphore: Arc<Semaphore>,
    state: Mutex<MeditationState>,
}

impl ScheduledMeditation {
    pub fn new() -> Self {
        Self {
            config: Mutex::new(None),
            db: Mutex::new(None),
            event_bus: Mutex::new(None),
            cancel_token: Mutex::new(None),
            scheduler: Mutex::new(None),
            condition_registry: Mutex::new(None),
            reflection_task_id: Mutex::new(None),
            habit_task_id: Mutex::new(None),
            enabled: AtomicBool::new(true),
            pending_tasks: AtomicU32::new(0),
            concurrency_semaphore: Arc::new(Semaphore::new(1)),
            state: Mutex::new(MeditationState {
                last_phase: None,
                last_executed_at: None,
                last_interaction_at: None,
                conditions_override: None,
            }),
        }
    }

    /// Execute a meditation phase, guarded by the concurrency semaphore
    async fn execute_phase(
        db: &DatabaseConnection,
        event_bus: &EventBus,
        user_id: Uuid,
        phase: &MeditationPhase,
        trigger: &str,
        semaphore: &Semaphore,
    ) -> anyhow::Result<MeditationLog> {
        let _permit = semaphore
            .acquire()
            .await
            .map_err(|_| anyhow::anyhow!("meditation semaphore closed"))?;

        let phase_str = phase.as_str();

        event_bus.publish(ServerEvent::MeditationStarted {
            phase: phase_str.to_string(),
            trigger: trigger.to_string(),
        });

        let start = std::time::Instant::now();
        let result = Self::run_phase(phase, user_id).await;
        let duration_ms = start.elapsed().as_millis() as i32;

        let log = match result {
            Ok(stats) => {
                let log = MeditationLog {
                    summary: stats.summary,
                    nodes_created: stats.nodes_created,
                    nodes_updated: stats.nodes_updated,
                    nodes_merged: stats.nodes_merged,
                    edges_created: stats.edges_created,
                    model_used: stats.model_used,
                    duration_ms,
                };

                event_bus.publish(ServerEvent::MeditationCompleted {
                    phase: phase_str.to_string(),
                    nodes_created: log.nodes_created as u32,
                    nodes_updated: log.nodes_updated as u32,
                });

                log
            }
            Err(e) => {
                tracing::error!(phase = phase_str, error = %e, "meditation phase failed");
                MeditationLog {
                    summary: format!("Phase failed: {e}"),
                    nodes_created: 0,
                    nodes_updated: 0,
                    nodes_merged: 0,
                    edges_created: 0,
                    model_used: None,
                    duration_ms,
                }
            }
        };

        // Persist log to database
        let log_id = Uuid::new_v4();
        let active_model = meditation_log::ActiveModel {
            id: Set(log_id),
            user_id: Set(user_id),
            phase: Set(phase_str.to_string()),
            trigger_type: Set(trigger.to_string()),
            summary: Set(log.summary.clone()),
            nodes_created: Set(log.nodes_created),
            nodes_updated: Set(log.nodes_updated),
            nodes_merged: Set(log.nodes_merged),
            edges_created: Set(log.edges_created),
            model_used: Set(log.model_used.clone()),
            duration_ms: Set(Some(log.duration_ms)),
            created_at: Set(Utc::now()),
        };
        if let Err(e) = active_model.insert(db).await {
            tracing::error!(error = %e, "failed to persist meditation log");
        }

        Ok(log)
    }

    /// Run a specific meditation phase
    /// TODO: Integrate with LLM and MemoryBackend for actual processing
    async fn run_phase(phase: &MeditationPhase, _user_id: Uuid) -> anyhow::Result<PhaseStats> {
        match phase {
            MeditationPhase::Extraction => {
                tracing::info!("executing Phase 1: memory extraction");
                // TODO: Use LlmAdapter to analyze recent conversation,
                //       extract facts/preferences/entities → store via MemoryBackend
                Ok(PhaseStats {
                    summary: "Extraction completed (placeholder)".to_string(),
                    nodes_created: 0,
                    nodes_updated: 0,
                    nodes_merged: 0,
                    edges_created: 0,
                    model_used: Some("local".to_string()),
                })
            }
            MeditationPhase::Consolidation => {
                tracing::info!("executing Phase 2: memory consolidation");
                // TODO: Summarize episodic → core, merge duplicates,
                //       apply time decay, run community detection via MemoryBackend
                Ok(PhaseStats {
                    summary: "Consolidation completed (placeholder)".to_string(),
                    nodes_created: 0,
                    nodes_updated: 0,
                    nodes_merged: 0,
                    edges_created: 0,
                    model_used: Some("local".to_string()),
                })
            }
            MeditationPhase::Reflection => {
                tracing::info!("executing Phase 3: self-reflection");
                // TODO: Use remote LLM to review recent memory changes,
                //       generate behavioral insights, store as core memories
                Ok(PhaseStats {
                    summary: "Reflection completed (placeholder)".to_string(),
                    nodes_created: 0,
                    nodes_updated: 0,
                    nodes_merged: 0,
                    edges_created: 0,
                    model_used: Some("remote".to_string()),
                })
            }
            MeditationPhase::HabitDetection => {
                tracing::info!("executing Phase 4: habit detection");
                // TODO: Analyze sensor data + interaction patterns,
                //       detect routines, update procedural memories
                Ok(PhaseStats {
                    summary: "Habit detection completed (placeholder)".to_string(),
                    nodes_created: 0,
                    nodes_updated: 0,
                    nodes_merged: 0,
                    edges_created: 0,
                    model_used: Some("remote".to_string()),
                })
            }
        }
    }

    /// Background loop: listens for events that trigger meditation phases.
    /// Handles both direct events (voice, sensor) and scheduler-fired TaskTriggered events.
    async fn event_listener_loop(
        db: DatabaseConnection,
        event_bus: EventBus,
        cancel_token: CancellationToken,
        enabled: Arc<AtomicBool>,
        pending_tasks: Arc<AtomicU32>,
        semaphore: Arc<Semaphore>,
        state: Arc<Mutex<MeditationState>>,
        config: MeditationSection,
        reflection_task_id: Option<Uuid>,
        habit_task_id: Option<Uuid>,
    ) {
        let mut rx = event_bus.subscribe();

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("meditation event listener cancelled");
                    break;
                }
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            if !enabled.load(Ordering::Relaxed) {
                                continue;
                            }
                            Self::handle_event(
                                &db, &event_bus, &event, &enabled,
                                &pending_tasks, &semaphore, &state, &config,
                                reflection_task_id, habit_task_id,
                            ).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(lagged = n, "meditation event listener lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::info!("event bus closed, stopping meditation listener");
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Handle incoming events that may trigger meditation
    async fn handle_event(
        db: &DatabaseConnection,
        event_bus: &EventBus,
        event: &ServerEvent,
        enabled: &AtomicBool,
        pending_tasks: &AtomicU32,
        semaphore: &Semaphore,
        state: &Mutex<MeditationState>,
        config: &MeditationSection,
        reflection_task_id: Option<Uuid>,
        habit_task_id: Option<Uuid>,
    ) {
        match event {
            // Scheduler-fired cron tasks for Phase 3 (reflection) and Phase 4 (habit detection)
            ServerEvent::TaskTriggered {
                task_id, task_name, ..
            } => {
                let phase = if Some(*task_id) == reflection_task_id
                    || task_name == TASK_NAME_REFLECTION
                {
                    Some(MeditationPhase::Reflection)
                } else if Some(*task_id) == habit_task_id || task_name == TASK_NAME_HABIT_DETECTION
                {
                    Some(MeditationPhase::HabitDetection)
                } else {
                    None
                };

                if let Some(phase) = phase {
                    let phase_str = phase.as_str();
                    let user_id = Uuid::nil(); // TODO: resolve user
                    pending_tasks.fetch_add(1, Ordering::Relaxed);
                    let _ =
                        Self::execute_phase(db, event_bus, user_id, &phase, "schedule", semaphore)
                            .await;
                    pending_tasks.fetch_sub(1, Ordering::Relaxed);

                    let mut s = state.lock().await;
                    s.last_phase = Some(phase_str.to_string());
                    s.last_executed_at = Some(Utc::now());
                }
            }

            // Phase 1: Post-conversation extraction
            // Triggered by VoiceStateChanged or a future ConversationEnded event
            ServerEvent::VoiceStateChanged { state: voice_state } if voice_state == "idle" => {
                if !config.extraction_enabled {
                    return;
                }
                // Update last interaction time
                {
                    let mut s = state.lock().await;
                    s.last_interaction_at = Some(Utc::now());
                }

                // TODO: Get actual user_id from conversation context
                let user_id = Uuid::nil();
                pending_tasks.fetch_add(1, Ordering::Relaxed);
                let db = db.clone();
                let event_bus = event_bus.clone();
                let semaphore_ref = semaphore;
                match Self::execute_phase(
                    &db,
                    &event_bus,
                    user_id,
                    &MeditationPhase::Extraction,
                    "post_conversation",
                    semaphore_ref,
                )
                .await
                {
                    Ok(log) => {
                        let mut s = state.lock().await;
                        s.last_phase = Some("extraction".to_string());
                        s.last_executed_at = Some(Utc::now());
                        tracing::info!(
                            summary = %log.summary,
                            duration_ms = log.duration_ms,
                            "extraction phase completed"
                        );
                    }
                    Err(e) => tracing::error!(error = %e, "extraction phase failed"),
                }
                pending_tasks.fetch_sub(1, Ordering::Relaxed);
            }

            // Sensor-triggered meditation: presence sensor detects user left/returned
            ServerEvent::SensorDataIngested {
                sensor_id, value, ..
            } => {
                if config.presence_sensor_id.is_empty() || sensor_id != &config.presence_sensor_id {
                    return;
                }

                let presence = value
                    .get("presence")
                    .and_then(|v| v.as_bool())
                    .or_else(|| value.as_bool())
                    .or_else(|| value.as_f64().map(|v| v > 0.0))
                    .unwrap_or(true);

                if presence {
                    // User returned — update interaction time
                    let mut s = state.lock().await;
                    s.last_interaction_at = Some(Utc::now());
                    return;
                }

                // User left — trigger consolidation + reflection
                if !enabled.load(Ordering::Relaxed) {
                    return;
                }

                let user_id = Uuid::nil(); // TODO: resolve from sensor context
                pending_tasks.fetch_add(1, Ordering::Relaxed);
                let _ = Self::execute_phase(
                    db,
                    event_bus,
                    user_id,
                    &MeditationPhase::Consolidation,
                    "sensor",
                    semaphore,
                )
                .await;
                let _ = Self::execute_phase(
                    db,
                    event_bus,
                    user_id,
                    &MeditationPhase::Reflection,
                    "sensor",
                    semaphore,
                )
                .await;
                pending_tasks.fetch_sub(1, Ordering::Relaxed);

                let mut s = state.lock().await;
                s.last_phase = Some("reflection".to_string());
                s.last_executed_at = Some(Utc::now());
            }

            _ => {}
        }
    }

    /// Background loop: checks for idle timeout to trigger consolidation
    async fn idle_check_loop(
        db: DatabaseConnection,
        event_bus: EventBus,
        cancel_token: CancellationToken,
        enabled: Arc<AtomicBool>,
        pending_tasks: Arc<AtomicU32>,
        semaphore: Arc<Semaphore>,
        state: Arc<Mutex<MeditationState>>,
        idle_minutes: u64,
    ) {
        let check_interval = std::time::Duration::from_secs(60); // Check every minute
        let mut interval = tokio::time::interval(check_interval);

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("meditation idle check loop cancelled");
                    break;
                }
                _ = interval.tick() => {
                    if !enabled.load(Ordering::Relaxed) {
                        continue;
                    }

                    let should_trigger = {
                        let s = state.lock().await;
                        if let Some(last_interaction) = s.last_interaction_at {
                            let idle_duration = Utc::now() - last_interaction;
                            let threshold = chrono::Duration::minutes(idle_minutes as i64);
                            idle_duration >= threshold
                        } else {
                            false
                        }
                    };

                    if should_trigger {
                        let user_id = Uuid::nil(); // TODO: resolve user
                        pending_tasks.fetch_add(1, Ordering::Relaxed);
                        let _ = Self::execute_phase(
                            &db, &event_bus, user_id,
                            &MeditationPhase::Consolidation, "idle", &semaphore,
                        ).await;
                        pending_tasks.fetch_sub(1, Ordering::Relaxed);

                        let mut s = state.lock().await;
                        s.last_phase = Some("consolidation".to_string());
                        s.last_executed_at = Some(Utc::now());
                        // Reset to avoid re-triggering until next interaction
                        s.last_interaction_at = None;
                    }
                }
            }
        }
    }

    /// Register cron-based meditation tasks with the SchedulerBackend
    async fn register_scheduler_tasks(
        scheduler: &dyn SchedulerBackend,
        config: &MeditationSection,
        conditions: &[serde_json::Value],
    ) -> anyhow::Result<(Option<Uuid>, Option<Uuid>)> {
        let conditions_json = serde_json::Value::Array(conditions.to_vec());

        // Phase 3: Reflection (e.g., "0 3 * * *")
        let reflection_task = ScheduledTask {
            id: Uuid::new_v4(),
            user_id: Uuid::nil(), // System-level task
            name: TASK_NAME_REFLECTION.to_string(),
            enabled: true,
            schedule: ScheduleConfig {
                schedule_type: "cron".to_string(),
                cron_expression: Some(config.reflection_schedule.clone()),
                run_at: None,
                every_sec: None,
            },
            conditions: conditions_json.clone(),
            action: serde_json::json!({
                "type": "meditation",
                "phase": "reflection",
            }),
            cooldown_sec: 300, // 5 min cooldown to avoid double-firing
            max_retries: 0,
            created_by: "system".to_string(),
        };
        let reflection_id = scheduler.create_task(reflection_task).await?;
        tracing::info!(
            task_id = %reflection_id,
            cron = %config.reflection_schedule,
            "registered meditation reflection task with scheduler"
        );

        // Phase 4: Habit detection (e.g., "0 4 * * *")
        let habit_task = ScheduledTask {
            id: Uuid::new_v4(),
            user_id: Uuid::nil(),
            name: TASK_NAME_HABIT_DETECTION.to_string(),
            enabled: true,
            schedule: ScheduleConfig {
                schedule_type: "cron".to_string(),
                cron_expression: Some(config.habit_detection_schedule.clone()),
                run_at: None,
                every_sec: None,
            },
            conditions: conditions_json,
            action: serde_json::json!({
                "type": "meditation",
                "phase": "habit_detection",
            }),
            cooldown_sec: 300,
            max_retries: 0,
            created_by: "system".to_string(),
        };
        let habit_id = scheduler.create_task(habit_task).await?;
        tracing::info!(
            task_id = %habit_id,
            cron = %config.habit_detection_schedule,
            "registered meditation habit detection task with scheduler"
        );

        Ok((Some(reflection_id), Some(habit_id)))
    }
}

impl Default for ScheduledMeditation {
    fn default() -> Self {
        Self::new()
    }
}

impl MeditationPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Extraction => "extraction",
            Self::Consolidation => "consolidation",
            Self::Reflection => "reflection",
            Self::HabitDetection => "habit_detection",
        }
    }
}

impl std::fmt::Display for MeditationPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Internal stats from a phase execution
struct PhaseStats {
    summary: String,
    nodes_created: i32,
    nodes_updated: i32,
    nodes_merged: i32,
    edges_created: i32,
    model_used: Option<String>,
}

/// Result of a meditation phase for internal tracking
struct MeditationLog {
    summary: String,
    nodes_created: i32,
    nodes_updated: i32,
    nodes_merged: i32,
    edges_created: i32,
    model_used: Option<String>,
    duration_ms: i32,
}

#[async_trait]
impl ModuleLifecycle for ScheduledMeditation {
    fn name(&self) -> &'static str {
        "scheduled"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let meditation_config = ctx.config.meditation.clone();

        self.enabled
            .store(meditation_config.enabled, Ordering::Relaxed);
        self.concurrency_semaphore = Arc::new(Semaphore::new(
            meditation_config.max_concurrent_meditations as usize,
        ));

        *self.config.lock().await = Some(meditation_config);
        *self.db.lock().await = Some(ctx.db.clone());
        *self.event_bus.lock().await = Some(ctx.event_bus.clone());
        *self.cancel_token.lock().await = Some(ctx.cancel_token.clone());
        *self.scheduler.lock().await = ctx.scheduler.clone();
        *self.condition_registry.lock().await = ctx.conditions.clone();

        tracing::info!("ScheduledMeditation initialized");
        Ok(())
    }

    async fn start(&self) -> anyhow::Result<()> {
        let config = self
            .config
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("meditation not initialized"))?;
        let db = self
            .db
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("meditation not initialized"))?;
        let event_bus = self
            .event_bus
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("meditation not initialized"))?;
        let cancel_token = self
            .cancel_token
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("meditation not initialized"))?;

        if !config.enabled {
            tracing::info!("meditation is disabled by config");
            return Ok(());
        }

        // Register cron tasks with the scheduler for Phase 3 + 4
        let conditions = config.conditions.clone();
        let (reflection_task_id, habit_task_id) = if let Some(scheduler) =
            self.scheduler.lock().await.as_ref()
        {
            match Self::register_scheduler_tasks(scheduler.as_ref(), &config, &conditions).await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!(error = %e, "failed to register meditation cron tasks");
                    (None, None)
                }
            }
        } else {
            tracing::warn!("no scheduler available, cron-based meditation phases disabled");
            (None, None)
        };

        *self.reflection_task_id.lock().await = reflection_task_id;
        *self.habit_task_id.lock().await = habit_task_id;

        let enabled = Arc::new(AtomicBool::new(self.enabled.load(Ordering::Relaxed)));
        let pending = Arc::new(AtomicU32::new(0));
        let semaphore = self.concurrency_semaphore.clone();
        let state = Arc::new(Mutex::new(MeditationState {
            last_phase: None,
            last_executed_at: None,
            last_interaction_at: Some(Utc::now()),
            conditions_override: None,
        }));

        // Spawn event listener (handles direct events + scheduler TaskTriggered)
        let db_clone = db.clone();
        let eb_clone = event_bus.clone();
        let ct_clone = cancel_token.clone();
        let en_clone = enabled.clone();
        let pt_clone = pending.clone();
        let sem_clone = semaphore.clone();
        let st_clone = state.clone();
        let cfg_clone = config.clone();
        tokio::spawn(async move {
            Self::event_listener_loop(
                db_clone,
                eb_clone,
                ct_clone,
                en_clone,
                pt_clone,
                sem_clone,
                st_clone,
                cfg_clone,
                reflection_task_id,
                habit_task_id,
            )
            .await;
        });

        // Spawn idle check loop
        let db_clone = db.clone();
        let eb_clone = event_bus.clone();
        let ct_clone = cancel_token.clone();
        let en_clone = enabled.clone();
        let pt_clone = pending.clone();
        let sem_clone = semaphore.clone();
        let st_clone = state.clone();
        let idle_minutes = config.consolidation_idle_minutes;
        tokio::spawn(async move {
            Self::idle_check_loop(
                db_clone,
                eb_clone,
                ct_clone,
                en_clone,
                pt_clone,
                sem_clone,
                st_clone,
                idle_minutes,
            )
            .await;
        });

        tracing::info!("ScheduledMeditation started (2 background tasks, cron via scheduler)");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        // Clean up scheduler tasks
        if let Some(scheduler) = self.scheduler.lock().await.as_ref() {
            if let Some(id) = *self.reflection_task_id.lock().await {
                let _ = scheduler.delete_task(id).await;
            }
            if let Some(id) = *self.habit_task_id.lock().await {
                let _ = scheduler.delete_task(id).await;
            }
        }
        tracing::info!("ScheduledMeditation stopped");
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if !self.enabled.load(Ordering::Relaxed) {
            return ModuleHealth::Degraded("meditation disabled".to_string());
        }
        ModuleHealth::Healthy
    }
}

#[async_trait]
impl MeditationStrategy for ScheduledMeditation {
    async fn schedule(&self, task: MeditationTask) -> anyhow::Result<()> {
        if !self.enabled.load(Ordering::Relaxed) {
            anyhow::bail!("meditation is disabled");
        }

        let phase = match task.phase.as_str() {
            "extraction" => MeditationPhase::Extraction,
            "consolidation" => MeditationPhase::Consolidation,
            "reflection" => MeditationPhase::Reflection,
            "habit_detection" => MeditationPhase::HabitDetection,
            other => anyhow::bail!("unknown meditation phase: {other}"),
        };

        let db = self
            .db
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("meditation not initialized"))?;
        let event_bus = self
            .event_bus
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("meditation not initialized"))?;

        self.pending_tasks.fetch_add(1, Ordering::Relaxed);

        let semaphore = self.concurrency_semaphore.clone();
        let pending = Arc::new(AtomicU32::new(self.pending_tasks.load(Ordering::Relaxed)));

        // Run in background to avoid blocking the caller
        tokio::spawn(async move {
            let _ = Self::execute_phase(
                &db,
                &event_bus,
                task.user_id,
                &phase,
                &task.trigger,
                &semaphore,
            )
            .await;
            pending.fetch_sub(1, Ordering::Relaxed);
        });

        Ok(())
    }

    async fn status(&self, user_id: Uuid) -> anyhow::Result<MeditationStatus> {
        let state = self.state.lock().await;

        // Query last log from DB for this user
        let last_log = if let Some(db) = self.db.lock().await.as_ref() {
            meditation_log::Entity::find()
                .filter(meditation_log::Column::UserId.eq(user_id))
                .order_by_desc(meditation_log::Column::CreatedAt)
                .one(db)
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        Ok(MeditationStatus {
            enabled: self.enabled.load(Ordering::Relaxed),
            last_phase: last_log
                .as_ref()
                .map(|l| l.phase.clone())
                .or_else(|| state.last_phase.clone()),
            last_executed_at: last_log
                .as_ref()
                .map(|l| l.created_at)
                .or(state.last_executed_at),
            pending_tasks: self.pending_tasks.load(Ordering::Relaxed),
        })
    }

    async fn trigger(&self, user_id: Uuid, phase: MeditationPhase) -> anyhow::Result<()> {
        let db = self
            .db
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("meditation not initialized"))?;
        let event_bus = self
            .event_bus
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("meditation not initialized"))?;

        self.pending_tasks.fetch_add(1, Ordering::Relaxed);
        let result = Self::execute_phase(
            &db,
            &event_bus,
            user_id,
            &phase,
            "manual",
            &self.concurrency_semaphore,
        )
        .await;
        self.pending_tasks.fetch_sub(1, Ordering::Relaxed);

        // Update internal state
        {
            let mut state = self.state.lock().await;
            state.last_phase = Some(phase.as_str().to_string());
            state.last_executed_at = Some(Utc::now());
        }

        result.map(|_| ())
    }

    async fn get_conditions(&self) -> Vec<serde_json::Value> {
        let state = self.state.lock().await;
        if let Some(overrides) = &state.conditions_override {
            return overrides.clone();
        }
        // Fall back to config defaults
        self.config
            .lock()
            .await
            .as_ref()
            .map(|c| c.conditions.clone())
            .unwrap_or_default()
    }

    async fn set_conditions(&self, conditions: Vec<serde_json::Value>) -> anyhow::Result<()> {
        // Store the override
        {
            let mut state = self.state.lock().await;
            state.conditions_override = Some(conditions.clone());
        }

        // Re-register scheduler tasks with updated conditions
        let scheduler = self.scheduler.lock().await;
        let config = self.config.lock().await;
        let (scheduler, config) = match (scheduler.as_ref(), config.as_ref()) {
            (Some(s), Some(c)) => (s, c),
            _ => return Ok(()), // No scheduler or config — nothing to update
        };

        // Delete old tasks
        if let Some(id) = *self.reflection_task_id.lock().await {
            let _ = scheduler.delete_task(id).await;
        }
        if let Some(id) = *self.habit_task_id.lock().await {
            let _ = scheduler.delete_task(id).await;
        }

        // Re-register with new conditions
        let (reflection_id, habit_id) =
            Self::register_scheduler_tasks(scheduler.as_ref(), config, &conditions).await?;
        *self.reflection_task_id.lock().await = reflection_id;
        *self.habit_task_id.lock().await = habit_id;

        tracing::info!(
            num_conditions = conditions.len(),
            "meditation conditions updated, scheduler tasks re-registered"
        );
        Ok(())
    }
}
