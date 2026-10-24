//! Scheduled background work with owned workers and bounded phase execution.

use crate::channels::adapter::ChannelEvent;
use crate::config::InsightsSection;
use crate::entity::{chat_session, insight_log};
use crate::event::{EventBus, ServerEvent};
use crate::modules::interaction::cache::review::{
    CacheReviewRuntime, review_pending_cache_entries,
};
use crate::modules::memory::MemoryService;
use crate::modules::memory::maintenance::{self, MemoryMaintenanceUser};
use crate::modules::platform::lifecycle::{ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    EmbeddingAdapter, ScheduleConfig, ScheduledTask, SchedulerBackend,
};
use crate::providers::llm::LlmService;
use crate::providers::llm::response::join_model_labels;
use crate::scheduler::ConditionRegistry;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{InsightPhase, InsightStatus, InsightTask};

/// Well-known task name prefix for insights cron tasks
const TASK_NAME_REFLECTION: &str = "insight_reflection";
const TASK_NAME_HABIT_DETECTION: &str = "insight_habit_detection";
// The same finite budget as a default scheduled task. It includes queueing,
// authority reads and provider work; no phase may monopolize the shared permit.
const PHASE_TIMEOUT: Duration = Duration::from_secs(30);
const LOG_TIMEOUT: Duration = Duration::from_secs(2);

/// The caller's future owns the count, including cancellation and early errors.
struct PendingTask<'a>(&'a AtomicU32);

impl<'a> PendingTask<'a> {
    fn new(counter: &'a AtomicU32) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}

impl Drop for PendingTask<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
/// Internal state tracking last execution and pending tasks
struct InsightState {
    last_phase: Option<String>,
    last_executed_at: Option<DateTime<Utc>>,
    last_interaction_at: Option<DateTime<Utc>>,
    active_users: HashMap<Uuid, MemoryMaintenanceUser>,
    /// User-level condition overrides; None = use config defaults
    conditions_override: Option<Vec<serde_json::Value>>,
}

pub struct InsightService {
    initialization: Option<InsightDependencies>,
    disabled: bool,
    config: Arc<Mutex<Option<InsightsSection>>>,
    db: Mutex<Option<DatabaseConnection>>,
    llm: Mutex<Option<Arc<LlmService>>>,
    memory: Mutex<Option<Arc<MemoryService>>>,
    cache_review: Mutex<CacheReviewRuntime>,
    event_bus: Mutex<Option<EventBus>>,
    cancel_token: Mutex<Option<CancellationToken>>,
    background: crate::modules::platform::background::OwnedBackground,
    scheduler_task_gate: Mutex<()>,
    scheduler: Mutex<Option<Arc<dyn SchedulerBackend>>>,
    condition_registry: Mutex<Option<Arc<ConditionRegistry>>>,
    /// Task IDs registered with the scheduler (for cleanup on stop)
    reflection_task_id: Mutex<Option<Uuid>>,
    habit_task_id: Mutex<Option<Uuid>>,
    enabled: Arc<AtomicBool>,
    pending_tasks: Arc<AtomicU32>,
    concurrency_semaphore: Arc<Semaphore>,
    state: Arc<Mutex<InsightState>>,
}

pub struct InsightDependencies {
    pub config: InsightsSection,
    pub db: DatabaseConnection,
    pub event_bus: EventBus,
    pub cancel_token: CancellationToken,
    pub llm: Option<Arc<LlmService>>,
    pub memory: Option<Arc<MemoryService>>,
    pub cache: Option<Arc<dyn crate::modules::interaction::cache::CacheStore>>,
    pub embedding: Option<Arc<dyn EmbeddingAdapter>>,
    pub scheduler: Option<Arc<dyn SchedulerBackend>>,
    pub conditions: Option<Arc<ConditionRegistry>>,
}

impl InsightService {
    pub(crate) fn check_available(enabled: bool) -> anyhow::Result<()> {
        anyhow::ensure!(
            !enabled || cfg!(feature = "insights"),
            "enabled insights require feature 'insights'"
        );
        Ok(())
    }

    pub fn new(enabled: bool) -> anyhow::Result<Self> {
        Self::check_available(enabled)?;
        Ok(Self {
            initialization: None,
            disabled: !enabled,
            config: Arc::new(Mutex::new(None)),
            db: Mutex::new(None),
            llm: Mutex::new(None),
            memory: Mutex::new(None),
            cache_review: Mutex::new(CacheReviewRuntime::default()),
            event_bus: Mutex::new(None),
            cancel_token: Mutex::new(None),
            background: crate::modules::platform::background::OwnedBackground::default(),
            scheduler_task_gate: Mutex::new(()),
            scheduler: Mutex::new(None),
            condition_registry: Mutex::new(None),
            reflection_task_id: Mutex::new(None),
            habit_task_id: Mutex::new(None),
            enabled: Arc::new(AtomicBool::new(enabled)),
            pending_tasks: Arc::new(AtomicU32::new(0)),
            concurrency_semaphore: Arc::new(Semaphore::new(1)),
            state: Arc::new(Mutex::new(InsightState {
                last_phase: None,
                last_executed_at: None,
                last_interaction_at: None,
                active_users: HashMap::new(),
                conditions_override: None,
            })),
        })
    }

    pub fn with_runtime(dependencies: InsightDependencies) -> anyhow::Result<Self> {
        let mut service = Self::new(dependencies.config.enabled)?;
        if !service.disabled {
            service.initialization = Some(dependencies);
        }
        Ok(service)
    }

    async fn bounded_phase<T>(
        semaphore: &Semaphore,
        phase: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        tokio::time::timeout(PHASE_TIMEOUT, async {
            let _permit = semaphore
                .acquire()
                .await
                .map_err(|_| anyhow::anyhow!("insights semaphore closed"))?;
            phase.await
        })
        .await
        .map_err(|_| anyhow::anyhow!(
            "insights phase deadline exceeded; completion is unconfirmed and earlier changes may already be committed"
        ))?
    }

    /// Execute an insights phase, guarded by a bounded concurrency permit.
    async fn execute_phase(
        db: &DatabaseConnection,
        event_bus: &EventBus,
        llm: Option<Arc<LlmService>>,
        memory: Option<Arc<MemoryService>>,
        cache_review: CacheReviewRuntime,
        user: MemoryMaintenanceUser,
        phase: &InsightPhase,
        trigger: &str,
        config: &InsightsSection,
        semaphore: &Semaphore,
    ) -> anyhow::Result<InsightLog> {
        let phase_str = phase.as_str();
        let start = tokio::time::Instant::now();
        let result = Self::bounded_phase(semaphore, async {
            // A queued event is not continuing account authorization.
            let account = tokio::time::timeout(
                LOG_TIMEOUT,
                crate::entity::user::Entity::find_by_id(user.user_id).one(db),
            )
            .await
            .map_err(|_| anyhow::anyhow!("insights account authorization timed out"))??;
            anyhow::ensure!(
                account.is_some_and(|account| account.status == "active"),
                "insights requires a currently active account"
            );
            event_bus.publish(ServerEvent::InsightStarted {
                phase: phase_str.to_string(),
                trigger: trigger.to_string(),
            });
            Self::run_phase(db, llm, memory, cache_review, phase, &user, config).await
        })
        .await;
        let duration_ms = start.elapsed().as_millis().min(i32::MAX as u128) as i32;

        let (log, failure) = match result {
            Ok(stats) => {
                let log = InsightLog {
                    summary: stats.summary,
                    nodes_created: stats.nodes_created,
                    nodes_updated: stats.nodes_updated,
                    nodes_merged: stats.nodes_merged,
                    edges_created: stats.edges_created,
                    model_used: stats.model_used,
                    duration_ms,
                };

                (log, None)
            }
            Err(e) => {
                tracing::error!(phase = phase_str, error = %e, "insights phase failed");
                let log = InsightLog {
                    summary: format!("Phase failed; earlier changes may already be committed: {e}"),
                    nodes_created: 0,
                    nodes_updated: 0,
                    nodes_merged: 0,
                    edges_created: 0,
                    model_used: None,
                    duration_ms,
                };
                (log, Some(e))
            }
        };

        // Persist log to database
        let log_id = Uuid::new_v4();
        let active_model = insight_log::ActiveModel {
            id: Set(log_id),
            user_id: Set(user.user_id),
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
        // Logging cannot keep the permit or this caller alive indefinitely.
        let persisted = tokio::time::timeout(LOG_TIMEOUT, active_model.insert(db))
            .await
            .map_err(|_| anyhow::anyhow!("insights log acknowledgement timed out"))
            .and_then(|result| result.map_err(anyhow::Error::from));
        if let Err(error) = persisted {
            return Err(anyhow::anyhow!(
                "insights log persistence failed; earlier phase changes may already be committed: {error}; phase failure: {}",
                failure
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "none".into())
            ));
        }

        if let Some(error) = failure {
            return Err(error
                .context("insights phase failed; earlier phase changes may already be committed"));
        }
        event_bus.publish(ServerEvent::InsightCompleted {
            phase: phase_str.to_string(),
            nodes_created: log.nodes_created as u32,
            nodes_updated: log.nodes_updated as u32,
        });
        Ok(log)
    }

    /// Run a specific insights phase.
    async fn run_phase(
        db: &DatabaseConnection,
        llm: Option<Arc<LlmService>>,
        memory: Option<Arc<MemoryService>>,
        cache_review: CacheReviewRuntime,
        phase: &InsightPhase,
        user: &MemoryMaintenanceUser,
        config: &InsightsSection,
    ) -> anyhow::Result<PhaseStats> {
        match phase {
            InsightPhase::Extraction => {
                tracing::info!("executing Phase 1: memory extraction");
                let stats = maintenance::extract_memories_from_recent_conversation(
                    db, memory, llm, user, 24, config,
                )
                .await?;
                Ok(PhaseStats {
                    summary: stats.summary,
                    nodes_created: stats.created as i32,
                    nodes_updated: stats.ingested as i32,
                    nodes_merged: 0,
                    edges_created: 0,
                    model_used: Some(stats.model_used),
                })
            }
            InsightPhase::Consolidation => {
                tracing::info!("executing Phase 2: memory consolidation");
                let memory_stats = maintenance::consolidate_memories(
                    db,
                    memory.clone(),
                    llm.clone(),
                    user,
                    50,
                    config,
                )
                .await?;
                let cache_stats =
                    review_pending_cache_entries(&cache_review, llm, user.user_id, 16).await?;
                Ok(PhaseStats {
                    summary: format!(
                        "{} Cache review approved {}, deleted {}, unconfirmed {}.",
                        memory_stats.summary,
                        cache_stats.approved,
                        cache_stats.deleted,
                        cache_stats.pending
                    ),
                    nodes_created: memory_stats.created as i32,
                    nodes_updated: (memory_stats.updated + cache_stats.approved) as i32,
                    nodes_merged: memory_stats.retired as i32,
                    edges_created: 0,
                    model_used: Some(join_model_labels(&[
                        memory_stats.model_used,
                        cache_stats.model_used,
                    ])),
                })
            }
            InsightPhase::Reflection => {
                tracing::info!("executing Phase 3: self-reflection");
                let memory_stats = maintenance::reflect_on_user_context(
                    db,
                    memory.clone(),
                    llm.clone(),
                    user,
                    40,
                    config,
                )
                .await?;
                let cache_stats =
                    review_pending_cache_entries(&cache_review, llm, user.user_id, 24).await?;
                Ok(PhaseStats {
                    summary: format!(
                        "{} Cache review approved {}, deleted {}, unconfirmed {}.",
                        memory_stats.summary,
                        cache_stats.approved,
                        cache_stats.deleted,
                        cache_stats.pending
                    ),
                    nodes_created: memory_stats.created as i32,
                    nodes_updated: cache_stats.approved as i32,
                    nodes_merged: 0,
                    edges_created: 0,
                    model_used: Some(join_model_labels(&[
                        memory_stats.model_used,
                        cache_stats.model_used,
                    ])),
                })
            }
            InsightPhase::HabitDetection => {
                tracing::info!("executing Phase 4: habit detection");
                let stats = maintenance::detect_habits_from_context(
                    db,
                    memory.clone(),
                    llm.clone(),
                    user,
                    40,
                    config,
                )
                .await?;
                Ok(PhaseStats {
                    summary: stats.summary,
                    nodes_created: stats.created as i32,
                    nodes_updated: 0,
                    nodes_merged: 0,
                    edges_created: 0,
                    model_used: Some(stats.model_used),
                })
            }
        }
    }

    /// Background loop: listens for events that trigger insights phases.
    /// Handles both direct events (voice, sensor) and scheduler-fired TaskTriggered events.
    async fn event_listener_loop(
        db: DatabaseConnection,
        event_bus: EventBus,
        llm: Option<Arc<LlmService>>,
        memory: Option<Arc<MemoryService>>,
        cache_review: CacheReviewRuntime,
        cancel_token: CancellationToken,
        enabled: Arc<AtomicBool>,
        pending_tasks: Arc<AtomicU32>,
        semaphore: Arc<Semaphore>,
        state: Arc<Mutex<InsightState>>,
        config: Arc<Mutex<Option<InsightsSection>>>,
        reflection_task_id: Option<Uuid>,
        habit_task_id: Option<Uuid>,
    ) {
        let mut rx = event_bus.subscribe();

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("insights event listener cancelled");
                    break;
                }
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            if !enabled.load(Ordering::Relaxed) {
                                continue;
                            }
                            let config = config.lock().await.clone().unwrap_or_default();
                            Self::handle_event(
                                &db, &event_bus, llm.clone(), memory.clone(), cache_review.clone(), &event, &enabled,
                                &pending_tasks, &semaphore, &state, &config,
                                reflection_task_id, habit_task_id,
                            ).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(lagged = n, "insights event listener lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::info!("event bus closed, stopping insights listener");
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Handle incoming events that may trigger insights
    async fn handle_event(
        db: &DatabaseConnection,
        event_bus: &EventBus,
        llm: Option<Arc<LlmService>>,
        memory: Option<Arc<MemoryService>>,
        cache_review: CacheReviewRuntime,
        event: &ServerEvent,
        enabled: &AtomicBool,
        pending_tasks: &AtomicU32,
        semaphore: &Semaphore,
        state: &Mutex<InsightState>,
        config: &InsightsSection,
        reflection_task_id: Option<Uuid>,
        habit_task_id: Option<Uuid>,
    ) {
        match event {
            ServerEvent::ChannelInput(event) => {
                if let Some(user) = Self::user_context_from_channel_event(event) {
                    Self::remember_user(state, user).await;
                }
            }
            ServerEvent::ConversationTurnCompleted {
                channel_id,
                user_id,
                user_uuid,
                session_id,
            } => {
                Self::remember_user(
                    state,
                    MemoryMaintenanceUser {
                        user_id: *user_uuid,
                        user_id_text: user_id.clone(),
                        channel_id: Some(channel_id.clone()),
                        session_id: Some(*session_id),
                        last_seen_at: Utc::now(),
                    },
                )
                .await;
            }
            // Scheduler-fired cron tasks for Phase 3 (reflection) and Phase 4 (habit detection)
            ServerEvent::TaskTriggered {
                task_id,
                user_id,
                task_name,
                ..
            } => {
                let phase = if Some(*task_id) == reflection_task_id
                    || task_name == TASK_NAME_REFLECTION
                {
                    Some(InsightPhase::Reflection)
                } else if Some(*task_id) == habit_task_id || task_name == TASK_NAME_HABIT_DETECTION
                {
                    Some(InsightPhase::HabitDetection)
                } else {
                    None
                };

                if let Some(phase) = phase {
                    let phase_str = phase.as_str();
                    let users = Self::resolve_phase_users(db, state, Some(*user_id)).await;
                    if users.is_empty() {
                        tracing::debug!(
                            phase = phase_str,
                            "no users available for scheduled insights task"
                        );
                        return;
                    }
                    let _pending = PendingTask::new(pending_tasks);
                    for user in users {
                        let _ = Self::execute_phase(
                            db,
                            event_bus,
                            llm.clone(),
                            memory.clone(),
                            cache_review.clone(),
                            user,
                            &phase,
                            "schedule",
                            config,
                            semaphore,
                        )
                        .await;
                    }

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

                let Some(user) = Self::last_active_user(state).await else {
                    tracing::debug!("voice idle insights extraction skipped without active user");
                    return;
                };
                let _pending = PendingTask::new(pending_tasks);
                let db = db.clone();
                let event_bus = event_bus.clone();
                let semaphore_ref = semaphore;
                match Self::execute_phase(
                    &db,
                    &event_bus,
                    llm.clone(),
                    memory.clone(),
                    cache_review.clone(),
                    user,
                    &InsightPhase::Extraction,
                    "post_conversation",
                    config,
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
            }

            // Sensor-triggered insights: presence sensor detects user left/returned
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

                let Some(user) = Self::last_active_user(state).await else {
                    tracing::debug!("sensor insights skipped without active user");
                    return;
                };
                let _pending = PendingTask::new(pending_tasks);
                let _ = Self::execute_phase(
                    db,
                    event_bus,
                    llm.clone(),
                    memory.clone(),
                    cache_review.clone(),
                    user.clone(),
                    &InsightPhase::Consolidation,
                    "sensor",
                    config,
                    semaphore,
                )
                .await;
                let _ = Self::execute_phase(
                    db,
                    event_bus,
                    llm.clone(),
                    memory.clone(),
                    cache_review.clone(),
                    user,
                    &InsightPhase::Reflection,
                    "sensor",
                    config,
                    semaphore,
                )
                .await;

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
        llm: Option<Arc<LlmService>>,
        memory: Option<Arc<MemoryService>>,
        cache_review: CacheReviewRuntime,
        cancel_token: CancellationToken,
        enabled: Arc<AtomicBool>,
        pending_tasks: Arc<AtomicU32>,
        semaphore: Arc<Semaphore>,
        state: Arc<Mutex<InsightState>>,
        config: Arc<Mutex<Option<InsightsSection>>>,
    ) {
        let check_interval = std::time::Duration::from_secs(60); // Check every minute
        let mut interval = tokio::time::interval(check_interval);

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("insights idle check loop cancelled");
                    break;
                }
                _ = interval.tick() => {
                    if !enabled.load(Ordering::Relaxed) {
                        continue;
                    }
                    let config = config.lock().await.clone().unwrap_or_default();
                    let idle_minutes = config.consolidation_idle_minutes;

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
                        let Some(user) = Self::last_active_user(&state).await else {
                            tracing::debug!("idle insights skipped without active user");
                            let mut s = state.lock().await;
                            s.last_interaction_at = None;
                            continue;
                        };
                        let _pending = PendingTask::new(&pending_tasks);
                        let _ = Self::execute_phase(
                            &db, &event_bus, llm.clone(), memory.clone(), cache_review.clone(), user,
                            &InsightPhase::Consolidation, "idle", &config, &semaphore,
                        ).await;

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

    fn user_context_from_channel_event(event: &ChannelEvent) -> Option<MemoryMaintenanceUser> {
        let (channel_id, user_id) = match event {
            ChannelEvent::TextMessage {
                channel_id,
                user_id,
                ..
            } => (channel_id, user_id),
            ChannelEvent::SensorData { channel_id, .. }
            | ChannelEvent::StatusChange { channel_id, .. } => {
                return Some(MemoryMaintenanceUser {
                    user_id: stable_user_uuid(channel_id),
                    user_id_text: channel_id.clone(),
                    channel_id: Some(channel_id.clone()),
                    session_id: None,
                    last_seen_at: Utc::now(),
                });
            }
        };

        Some(MemoryMaintenanceUser {
            user_id: stable_user_uuid(user_id),
            user_id_text: user_id.clone(),
            channel_id: Some(channel_id.clone()),
            session_id: None,
            last_seen_at: Utc::now(),
        })
    }

    async fn remember_user(state: &Mutex<InsightState>, user: MemoryMaintenanceUser) {
        let mut state = state.lock().await;
        state.last_interaction_at = Some(user.last_seen_at);
        state.active_users.insert(user.user_id, user);
    }

    async fn last_active_user(state: &Mutex<InsightState>) -> Option<MemoryMaintenanceUser> {
        let state = state.lock().await;
        state
            .active_users
            .values()
            .max_by_key(|user| user.last_seen_at)
            .cloned()
    }

    async fn resolve_phase_users(
        db: &DatabaseConnection,
        state: &Mutex<InsightState>,
        requested_user_id: Option<Uuid>,
    ) -> Vec<MemoryMaintenanceUser> {
        if let Some(user_id) = requested_user_id.filter(|id| !id.is_nil()) {
            if let Some(user) = Self::active_user_by_id(state, user_id).await {
                return vec![user];
            }
            if let Some(user) = Self::load_latest_user_context(db, user_id).await {
                return vec![user];
            }
            return vec![MemoryMaintenanceUser {
                user_id,
                user_id_text: user_id.to_string(),
                channel_id: None,
                session_id: None,
                last_seen_at: Utc::now(),
            }];
        }

        let mut users = {
            let state = state.lock().await;
            state.active_users.values().cloned().collect::<Vec<_>>()
        };
        if users.is_empty() {
            users = Self::load_recent_user_contexts(db, 20).await;
        }
        users.sort_by_key(|user| std::cmp::Reverse(user.last_seen_at));
        users.truncate(20);
        users
    }

    async fn active_user_by_id(
        state: &Mutex<InsightState>,
        user_id: Uuid,
    ) -> Option<MemoryMaintenanceUser> {
        state.lock().await.active_users.get(&user_id).cloned()
    }

    async fn load_latest_user_context(
        db: &DatabaseConnection,
        user_id: Uuid,
    ) -> Option<MemoryMaintenanceUser> {
        let user_id_text = user_id.to_string();
        let session = chat_session::Entity::find()
            .filter(chat_session::Column::CreatorId.eq(Some(user_id_text.clone())))
            .order_by_desc(chat_session::Column::UpdatedAt)
            .limit(1)
            .one(db)
            .await
            .ok()
            .flatten()?;

        Some(MemoryMaintenanceUser {
            user_id,
            user_id_text,
            channel_id: Some(session.channel_id),
            session_id: Some(session.id),
            last_seen_at: session.updated_at.into(),
        })
    }

    async fn load_recent_user_contexts(
        db: &DatabaseConnection,
        limit: u64,
    ) -> Vec<MemoryMaintenanceUser> {
        let sessions = chat_session::Entity::find()
            .filter(chat_session::Column::CreatorId.is_not_null())
            .order_by_desc(chat_session::Column::UpdatedAt)
            .limit(limit.saturating_mul(3).max(1))
            .all(db)
            .await
            .unwrap_or_default();

        let mut users = HashMap::new();
        for session in sessions {
            let Some(user_id_text) = session.creator_id.clone() else {
                continue;
            };
            let user_id = stable_user_uuid(&user_id_text);
            users
                .entry(user_id)
                .or_insert_with(|| MemoryMaintenanceUser {
                    user_id,
                    user_id_text,
                    channel_id: Some(session.channel_id),
                    session_id: Some(session.id),
                    last_seen_at: session.updated_at.into(),
                });
            if users.len() >= limit as usize {
                break;
            }
        }
        users.into_values().collect()
    }

    /// Register cron-based insights tasks with the SchedulerBackend
    async fn register_scheduler_tasks(
        &self,
        scheduler: &dyn SchedulerBackend,
        config: &InsightsSection,
        conditions: &[serde_json::Value],
    ) -> anyhow::Result<(Option<Uuid>, Option<Uuid>)> {
        let conditions_json = serde_json::Value::Array(conditions.to_vec());

        // Phase 3: Reflection (e.g., "0 0 3 * * *"; 5-field Unix cron is accepted too)
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
                poll_interval_sec: None,
            },
            conditions: conditions_json.clone(),
            action: serde_json::json!({
                "type": "insights",
                "phase": "reflection",
            }),
            cooldown_sec: 300, // 5 min cooldown to avoid double-firing
            max_retries: 0,
            created_by: "system".to_string(),
        };
        // Retain the identity before the DB await: a committed write can lose its ACK.
        *self.reflection_task_id.lock().await = Some(reflection_task.id);
        let reflection_id = scheduler.create_task(reflection_task).await?;
        tracing::info!(
            task_id = %reflection_id,
            cron = %config.reflection_schedule,
            "registered insights reflection task with scheduler"
        );

        // Phase 4: Habit detection (e.g., "0 0 4 * * *"; 5-field Unix cron is accepted too)
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
                poll_interval_sec: None,
            },
            conditions: conditions_json,
            action: serde_json::json!({
                "type": "insights",
                "phase": "habit_detection",
            }),
            cooldown_sec: 300,
            max_retries: 0,
            created_by: "system".to_string(),
        };
        *self.habit_task_id.lock().await = Some(habit_task.id);
        let habit_id = scheduler.create_task(habit_task).await?;
        tracing::info!(
            task_id = %habit_id,
            cron = %config.habit_detection_schedule,
            "registered insights habit detection task with scheduler"
        );

        Ok((Some(reflection_id), Some(habit_id)))
    }
}
fn stable_user_uuid(user_id: &str) -> Uuid {
    let user_id = user_id.trim();
    let stable_key = if user_id.is_empty() {
        "unknown-insights-user"
    } else {
        user_id
    };
    Uuid::parse_str(stable_key).unwrap_or_else(|_| {
        let digest = Sha256::digest(stable_key.as_bytes());
        let mut bytes = [
            digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
            digest[8], digest[9], digest[10], digest[11], digest[12], digest[13], digest[14],
            digest[15],
        ];
        bytes[6] = (bytes[6] & 0x0f) | 0x50;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid::from_bytes(bytes)
    })
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

/// Result of an insights phase for internal tracking
struct InsightLog {
    summary: String,
    nodes_created: i32,
    nodes_updated: i32,
    nodes_merged: i32,
    edges_created: i32,
    model_used: Option<String>,
    duration_ms: i32,
}

#[async_trait]
impl ModuleLifecycle for InsightService {
    fn name(&self) -> &'static str {
        if self.disabled {
            return "noop";
        }
        "scheduled"
    }

    async fn init(&mut self) -> anyhow::Result<()> {
        if self.disabled {
            return Ok(());
        }
        let dependencies = self
            .initialization
            .take()
            .ok_or_else(|| anyhow::anyhow!("insights initialization dependencies are not bound"))?;
        let insights_config = dependencies.config;

        self.enabled
            .store(insights_config.enabled, Ordering::Relaxed);
        self.concurrency_semaphore = Arc::new(Semaphore::new(
            insights_config.max_concurrent_insights as usize,
        ));

        *self.config.lock().await = Some(insights_config);
        *self.db.lock().await = Some(dependencies.db);
        *self.llm.lock().await = dependencies.llm;
        *self.memory.lock().await = dependencies.memory;
        *self.cache_review.lock().await = CacheReviewRuntime {
            cache: dependencies.cache,
            embedding: dependencies.embedding,
        };
        *self.event_bus.lock().await = Some(dependencies.event_bus);
        *self.cancel_token.lock().await = Some(dependencies.cancel_token);
        *self.scheduler.lock().await = dependencies.scheduler;
        *self.condition_registry.lock().await = dependencies.conditions;

        tracing::info!("InsightService initialized");
        Ok(())
    }

    async fn start(&self) -> anyhow::Result<()> {
        if self.disabled {
            return Ok(());
        }
        let config = self
            .config
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("insights not initialized"))?;
        let db = self
            .db
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("insights not initialized"))?;
        let event_bus = self
            .event_bus
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("insights not initialized"))?;
        let cancel_token = self
            .cancel_token
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("insights not initialized"))?;
        let mut background = self.background.begin_start(&cancel_token).await?;
        let cancel_token = background.token();
        let _tasks = self.scheduler_task_gate.lock().await;
        let llm = self.llm.lock().await.clone();
        let memory = self.memory.lock().await.clone();
        let cache_review = self.cache_review.lock().await.clone();

        if !config.enabled {
            background.commit();
            tracing::info!("insights is disabled by config");
            return Ok(());
        }

        // Register cron tasks with the scheduler for Phase 3 + 4
        let conditions = config.conditions.clone();
        let (reflection_task_id, habit_task_id) = if let Some(scheduler) =
            self.scheduler.lock().await.as_ref()
        {
            tokio::select! {
                biased;
                _ = cancel_token.cancelled() => anyhow::bail!("insights startup cancelled"),
                result = self.register_scheduler_tasks(scheduler.as_ref(), &config, &conditions) => result?,
            }
        } else {
            tracing::warn!("no scheduler available, cron-based insights phases disabled");
            (None, None)
        };

        *self.reflection_task_id.lock().await = reflection_task_id;
        *self.habit_task_id.lock().await = habit_task_id;

        let enabled = self.enabled.clone();
        let pending = self.pending_tasks.clone();
        let semaphore = self.concurrency_semaphore.clone();
        {
            let mut state = self.state.lock().await;
            state.last_interaction_at = Some(Utc::now());
        }
        let state = self.state.clone();

        // Spawn event listener (handles direct events + scheduler TaskTriggered)
        let db_clone = db.clone();
        let eb_clone = event_bus.clone();
        let ct_clone = cancel_token.clone();
        let en_clone = enabled.clone();
        let pt_clone = pending.clone();
        let sem_clone = semaphore.clone();
        let st_clone = state.clone();
        let cfg_clone = self.config.clone();
        let listener_llm = llm.clone();
        let listener_memory = memory.clone();
        let listener_cache_review = cache_review.clone();
        background.spawn_loop("insights events", async move {
            Self::event_listener_loop(
                db_clone,
                eb_clone,
                listener_llm,
                listener_memory,
                listener_cache_review,
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
        let idle_config = self.config.clone();
        background.spawn_loop("insights idle checks", async move {
            Self::idle_check_loop(
                db_clone,
                eb_clone,
                llm,
                memory,
                cache_review,
                ct_clone,
                en_clone,
                pt_clone,
                sem_clone,
                st_clone,
                idle_config,
            )
            .await;
        });

        background.commit();
        tracing::info!("InsightService started (2 background tasks, cron via scheduler)");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        if self.disabled {
            return Ok(());
        }
        self.background.request_stop();
        let mut errors = Vec::new();
        if let Err(error) = self.background.stop().await {
            errors.push(format!("workers: {error:#}"));
        }
        let _tasks = self.scheduler_task_gate.lock().await;
        if let Some(scheduler) = self.scheduler.lock().await.as_ref() {
            for task in [&self.reflection_task_id, &self.habit_task_id] {
                let mut id = task.lock().await;
                if let Some(task_id) = *id {
                    match scheduler.delete_task(task_id).await {
                        Ok(()) => *id = None,
                        Err(error) => errors.push(format!("scheduler task {task_id}: {error:#}")),
                    }
                }
            }
        }
        anyhow::ensure!(
            errors.is_empty(),
            "insights shutdown incomplete: {}",
            errors.join("; ")
        );
        tracing::info!("InsightService stopped");
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if self.disabled {
            return ModuleHealth::Healthy;
        }
        if !self.enabled.load(Ordering::Relaxed) {
            return ModuleHealth::Degraded("insights disabled".to_string());
        }
        ModuleHealth::Healthy
    }

    async fn apply_settings(
        &self,
        updates: &std::collections::BTreeMap<String, serde_json::Value>,
    ) -> anyhow::Result<Vec<String>> {
        if self.disabled {
            return Ok(vec![]);
        }
        let mut applied = Vec::new();
        let mut config = self.config.lock().await;
        let Some(config) = config.as_mut() else {
            return Ok(applied);
        };

        apply_prompt_setting(
            updates,
            "core.policies.insights.memory_extraction_prompt",
            &mut config.memory_extraction_prompt,
            &mut applied,
        );
        apply_prompt_setting(
            updates,
            "core.policies.insights.memory_consolidation_prompt",
            &mut config.memory_consolidation_prompt,
            &mut applied,
        );
        apply_prompt_setting(
            updates,
            "core.policies.insights.reflection_prompt",
            &mut config.reflection_prompt,
            &mut applied,
        );
        apply_prompt_setting(
            updates,
            "core.policies.insights.habit_detection_prompt",
            &mut config.habit_detection_prompt,
            &mut applied,
        );

        Ok(applied)
    }
}

fn apply_prompt_setting(
    updates: &std::collections::BTreeMap<String, serde_json::Value>,
    key: &str,
    target: &mut String,
    applied: &mut Vec<String>,
) {
    let Some(value) = updates.get(key).and_then(serde_json::Value::as_str) else {
        return;
    };
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    *target = value.to_string();
    applied.push(key.to_string());
}

impl InsightService {
    pub async fn schedule(&self, task: InsightTask) -> anyhow::Result<()> {
        if self.disabled {
            return Ok(());
        }
        anyhow::ensure!(!self.background.is_stopping(), "insights is stopping");
        if !self.enabled.load(Ordering::Relaxed) {
            anyhow::bail!("insights is disabled");
        }

        let phase = match task.phase.as_str() {
            "extraction" => InsightPhase::Extraction,
            "consolidation" => InsightPhase::Consolidation,
            "reflection" => InsightPhase::Reflection,
            "habit_detection" => InsightPhase::HabitDetection,
            other => anyhow::bail!("unknown insights phase: {other}"),
        };

        let db = self
            .db
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("insights not initialized"))?;
        let event_bus = self
            .event_bus
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("insights not initialized"))?;
        let config = self
            .config
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("insights not initialized"))?;
        let llm = self.llm.lock().await.clone();
        let memory = self.memory.lock().await.clone();
        let cache_review = self.cache_review.lock().await.clone();
        let users = Self::resolve_phase_users(&db, &self.state, Some(task.user_id)).await;
        if users.is_empty() {
            tracing::debug!(
                phase = %task.phase,
                trigger = %task.trigger,
                "insights schedule skipped because no user context is available"
            );
            return Ok(());
        }

        let _pending = PendingTask::new(&self.pending_tasks);

        for user in users {
            Self::execute_phase(
                &db,
                &event_bus,
                llm.clone(),
                memory.clone(),
                cache_review.clone(),
                user,
                &phase,
                &task.trigger,
                &config,
                &self.concurrency_semaphore,
            )
            .await?;
        }

        Ok(())
    }

    pub async fn status(&self, user_id: Uuid) -> anyhow::Result<InsightStatus> {
        if self.disabled {
            return Ok(InsightStatus {
                enabled: false,
                last_phase: None,
                last_executed_at: None,
                pending_tasks: 0,
            });
        }
        let state = self.state.lock().await;

        // Query last log from DB for this user
        let last_log = if let Some(db) = self.db.lock().await.as_ref() {
            insight_log::Entity::find()
                .filter(insight_log::Column::UserId.eq(user_id))
                .order_by_desc(insight_log::Column::CreatedAt)
                .one(db)
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        Ok(InsightStatus {
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

    pub async fn trigger(&self, user_id: Uuid, phase: InsightPhase) -> anyhow::Result<()> {
        if self.disabled {
            return Ok(());
        }
        anyhow::ensure!(!self.background.is_stopping(), "insights is stopping");
        anyhow::ensure!(self.enabled.load(Ordering::Relaxed), "insights is disabled");
        let db = self
            .db
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("insights not initialized"))?;
        let event_bus = self
            .event_bus
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("insights not initialized"))?;
        let config = self
            .config
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("insights not initialized"))?;
        let llm = self.llm.lock().await.clone();
        let memory = self.memory.lock().await.clone();
        let cache_review = self.cache_review.lock().await.clone();
        let user = Self::resolve_phase_users(&db, &self.state, Some(user_id))
            .await
            .into_iter()
            .next()
            .unwrap_or_else(|| MemoryMaintenanceUser {
                user_id,
                user_id_text: user_id.to_string(),
                channel_id: None,
                session_id: None,
                last_seen_at: Utc::now(),
            });

        let _pending = PendingTask::new(&self.pending_tasks);
        let result = Self::execute_phase(
            &db,
            &event_bus,
            llm,
            memory,
            cache_review,
            user,
            &phase,
            "manual",
            &config,
            &self.concurrency_semaphore,
        )
        .await;

        // Update internal state
        {
            let mut state = self.state.lock().await;
            state.last_phase = Some(phase.as_str().to_string());
            state.last_executed_at = Some(Utc::now());
        }

        result.map(|_| ())
    }

    pub async fn get_conditions(&self) -> Vec<serde_json::Value> {
        if self.disabled {
            return vec![];
        }
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

    pub async fn set_conditions(&self, conditions: Vec<serde_json::Value>) -> anyhow::Result<()> {
        if self.disabled {
            return Ok(());
        }
        let _tasks = self.scheduler_task_gate.lock().await;
        anyhow::ensure!(!self.background.is_stopping(), "insights is stopping");
        tokio::select! {
            biased;
            _ = self.background.cancelled() => anyhow::bail!("insights is stopping"),
            result = async {
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
                    scheduler.delete_task(id).await?;
                }
                if let Some(id) = *self.habit_task_id.lock().await {
                    scheduler.delete_task(id).await?;
                }

                // Re-register with new conditions
                let (reflection_id, habit_id) =
                    self.register_scheduler_tasks(scheduler.as_ref(), config, &conditions).await?;
                *self.reflection_task_id.lock().await = reflection_id;
                *self.habit_task_id.lock().await = habit_id;

                tracing::info!(
                    num_conditions = conditions.len(),
                    "insights conditions updated, scheduler tasks re-registered"
                );
                Ok(())
            } => result,
        }
    }
}

#[cfg(test)]
mod tests;
