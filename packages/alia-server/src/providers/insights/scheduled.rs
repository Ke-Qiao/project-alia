//! ScheduledInsights: Full implementation of InsightStrategy.
//!
//! Triggers insights phases via:
//! - Event-driven: completed/idle conversation turns → Phase 1 (extraction)
//! - Event-driven: Idle detection → Phase 2 (consolidation)
//! - Scheduler: reflection_schedule → Phase 3 (reflection) via AutomationScheduler
//! - Scheduler: habit_detection_schedule → Phase 4 (habit detection) via AutomationScheduler
//! - Manual trigger via API

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, Statement,
};
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::channels::adapter::ChannelEvent;
use crate::config::InsightsSection;
use crate::entity::{chat_session, insight_log, llm_cache};
use crate::event::{EventBus, ServerEvent};
use crate::memory_policy::detect_sensitive_memory;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    CACHE_METADATA_SCHEMA, ChatMessage, InsightPhase, InsightStatus, InsightStrategy, InsightTask,
    LlmAdapter, MemoryBackend, MemoryBatch, MemoryListOpts, MemoryNode, MemoryPatch,
    MemoryWriteContext, ScheduleConfig, ScheduledTask, SchedulerBackend,
};
use crate::scheduler::ConditionRegistry;

/// Well-known task name prefix for insights cron tasks
const TASK_NAME_REFLECTION: &str = "insight_reflection";
const TASK_NAME_HABIT_DETECTION: &str = "insight_habit_detection";
const CACHE_STATUS_PENDING: &str = "pending_insights_review";
const CACHE_STATUS_ACTIVE: &str = "active";

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

#[derive(Debug, Clone)]
struct PendingCacheEntry {
    id: Uuid,
    query_text: String,
    response_text: String,
    metadata: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
struct CacheReviewStats {
    approved: u32,
    deleted: u32,
    pending: u32,
    model_used: String,
}

#[derive(Debug, Deserialize)]
struct CacheReviewDecision {
    decision: String,
    #[serde(default)]
    refined_response: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    safe_direct_return: Option<bool>,
    #[serde(default)]
    memory_independent: Option<bool>,
    #[serde(default)]
    tool_free: Option<bool>,
    #[serde(default)]
    dynamic_free: Option<bool>,
}

#[derive(Debug, Clone)]
struct InsightUserContext {
    user_id: Uuid,
    user_id_text: String,
    channel_id: Option<String>,
    session_id: Option<Uuid>,
    last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
struct MemoryInsightStats {
    created: u32,
    updated: u32,
    retired: u32,
    ingested: u32,
    pending: u32,
    summary: String,
    model_used: String,
}

#[derive(Debug, Deserialize)]
struct MemoryExtractionResponse {
    #[serde(default)]
    memories: Vec<MemoryCandidate>,
}

#[derive(Debug, Deserialize)]
struct MemoryCandidate {
    content: String,
    #[serde(default)]
    memory_layer: Option<String>,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    node_type: Option<String>,
    #[serde(default)]
    importance: Option<f32>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryConsolidationResponse {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    updates: Vec<MemoryUpdateDecision>,
    #[serde(default)]
    retirements: Vec<MemoryRetirementDecision>,
    #[serde(default)]
    creates: Vec<MemoryCandidate>,
}

#[derive(Debug, Deserialize)]
struct MemoryUpdateDecision {
    id: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    importance: Option<f32>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryRetirementDecision {
    id: String,
    status: RetirementStatus,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RetirementStatus {
    Superseded,
    Rejected,
}

/// Internal state tracking last execution and pending tasks
struct InsightState {
    last_phase: Option<String>,
    last_executed_at: Option<DateTime<Utc>>,
    last_interaction_at: Option<DateTime<Utc>>,
    active_users: HashMap<Uuid, InsightUserContext>,
    /// User-level condition overrides; None = use config defaults
    conditions_override: Option<Vec<serde_json::Value>>,
}

pub struct ScheduledInsights {
    config: Arc<Mutex<Option<InsightsSection>>>,
    db: Mutex<Option<DatabaseConnection>>,
    llm: Mutex<Option<Arc<dyn LlmAdapter>>>,
    memory: Mutex<Option<Arc<dyn MemoryBackend>>>,
    event_bus: Mutex<Option<EventBus>>,
    cancel_token: Mutex<Option<CancellationToken>>,
    background: crate::module::background::OwnedBackground,
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

impl ScheduledInsights {
    pub fn new() -> Self {
        Self {
            config: Arc::new(Mutex::new(None)),
            db: Mutex::new(None),
            llm: Mutex::new(None),
            memory: Mutex::new(None),
            event_bus: Mutex::new(None),
            cancel_token: Mutex::new(None),
            background: crate::module::background::OwnedBackground::default(),
            scheduler_task_gate: Mutex::new(()),
            scheduler: Mutex::new(None),
            condition_registry: Mutex::new(None),
            reflection_task_id: Mutex::new(None),
            habit_task_id: Mutex::new(None),
            enabled: Arc::new(AtomicBool::new(true)),
            pending_tasks: Arc::new(AtomicU32::new(0)),
            concurrency_semaphore: Arc::new(Semaphore::new(1)),
            state: Arc::new(Mutex::new(InsightState {
                last_phase: None,
                last_executed_at: None,
                last_interaction_at: None,
                active_users: HashMap::new(),
                conditions_override: None,
            })),
        }
    }

    /// Execute an insights phase, guarded by the concurrency semaphore
    async fn execute_phase(
        db: &DatabaseConnection,
        event_bus: &EventBus,
        llm: Option<Arc<dyn LlmAdapter>>,
        memory: Option<Arc<dyn MemoryBackend>>,
        user: InsightUserContext,
        phase: &InsightPhase,
        trigger: &str,
        config: &InsightsSection,
        semaphore: &Semaphore,
    ) -> anyhow::Result<InsightLog> {
        let _permit = semaphore
            .acquire()
            .await
            .map_err(|_| anyhow::anyhow!("insights semaphore closed"))?;

        // Recent interaction context is not proof that the account is still active.
        // Recheck after waiting for capacity and before reading or sending any context.
        let account = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            crate::entity::user::Entity::find_by_id(user.user_id).one(db),
        )
        .await
        .map_err(|_| anyhow::anyhow!("insights account authorization timed out"))??;
        anyhow::ensure!(
            account.is_some_and(|account| account.status == "active"),
            "insights requires a currently active account"
        );

        let phase_str = phase.as_str();

        event_bus.publish(ServerEvent::InsightStarted {
            phase: phase_str.to_string(),
            trigger: trigger.to_string(),
        });

        let start = std::time::Instant::now();
        let result = Self::run_phase(db, llm, memory, phase, &user, config).await;
        let duration_ms = start.elapsed().as_millis() as i32;

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
        if let Err(error) = active_model.insert(db).await {
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
        llm: Option<Arc<dyn LlmAdapter>>,
        memory: Option<Arc<dyn MemoryBackend>>,
        phase: &InsightPhase,
        user: &InsightUserContext,
        config: &InsightsSection,
    ) -> anyhow::Result<PhaseStats> {
        match phase {
            InsightPhase::Extraction => {
                tracing::info!("executing Phase 1: memory extraction");
                let stats = Self::extract_memories_from_recent_conversation(
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
                let memory_stats =
                    Self::consolidate_memories(memory.clone(), llm.clone(), user, 50, config)
                        .await?;
                let cache_stats =
                    Self::review_pending_cache_entries(db, llm, user.user_id, 16).await?;
                Ok(PhaseStats {
                    summary: format!(
                        "{} Cache review approved {}, deleted {}, left pending {}.",
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
                let memory_stats = Self::reflect_on_user_context(
                    db,
                    memory.clone(),
                    llm.clone(),
                    user,
                    40,
                    config,
                )
                .await?;
                let cache_stats =
                    Self::review_pending_cache_entries(db, llm, user.user_id, 24).await?;
                Ok(PhaseStats {
                    summary: format!(
                        "{} Cache review approved {}, deleted {}, left pending {}.",
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
                let stats = Self::detect_habits_from_context(
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
        llm: Option<Arc<dyn LlmAdapter>>,
        memory: Option<Arc<dyn MemoryBackend>>,
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
                                &db, &event_bus, llm.clone(), memory.clone(), &event, &enabled,
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
        llm: Option<Arc<dyn LlmAdapter>>,
        memory: Option<Arc<dyn MemoryBackend>>,
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
                    InsightUserContext {
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
        llm: Option<Arc<dyn LlmAdapter>>,
        memory: Option<Arc<dyn MemoryBackend>>,
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
                            &db, &event_bus, llm.clone(), memory.clone(), user,
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

    fn user_context_from_channel_event(event: &ChannelEvent) -> Option<InsightUserContext> {
        let (channel_id, user_id) = match event {
            ChannelEvent::TextMessage {
                channel_id,
                user_id,
                ..
            }
            | ChannelEvent::AudioInput {
                channel_id,
                user_id,
                ..
            }
            | ChannelEvent::InterruptActiveTurn {
                channel_id,
                user_id,
                ..
            } => (channel_id, user_id),
            ChannelEvent::SensorData { channel_id, .. }
            | ChannelEvent::StatusChange { channel_id, .. } => {
                return Some(InsightUserContext {
                    user_id: stable_user_uuid(channel_id),
                    user_id_text: channel_id.clone(),
                    channel_id: Some(channel_id.clone()),
                    session_id: None,
                    last_seen_at: Utc::now(),
                });
            }
        };

        Some(InsightUserContext {
            user_id: stable_user_uuid(user_id),
            user_id_text: user_id.clone(),
            channel_id: Some(channel_id.clone()),
            session_id: None,
            last_seen_at: Utc::now(),
        })
    }

    async fn remember_user(state: &Mutex<InsightState>, user: InsightUserContext) {
        let mut state = state.lock().await;
        state.last_interaction_at = Some(user.last_seen_at);
        state.active_users.insert(user.user_id, user);
    }

    async fn last_active_user(state: &Mutex<InsightState>) -> Option<InsightUserContext> {
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
    ) -> Vec<InsightUserContext> {
        if let Some(user_id) = requested_user_id.filter(|id| !id.is_nil()) {
            if let Some(user) = Self::active_user_by_id(state, user_id).await {
                return vec![user];
            }
            if let Some(user) = Self::load_latest_user_context(db, user_id).await {
                return vec![user];
            }
            return vec![InsightUserContext {
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
    ) -> Option<InsightUserContext> {
        state.lock().await.active_users.get(&user_id).cloned()
    }

    async fn load_latest_user_context(
        db: &DatabaseConnection,
        user_id: Uuid,
    ) -> Option<InsightUserContext> {
        let user_id_text = user_id.to_string();
        let session = chat_session::Entity::find()
            .filter(chat_session::Column::CreatorId.eq(Some(user_id_text.clone())))
            .order_by_desc(chat_session::Column::UpdatedAt)
            .limit(1)
            .one(db)
            .await
            .ok()
            .flatten()?;

        Some(InsightUserContext {
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
    ) -> Vec<InsightUserContext> {
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
            users.entry(user_id).or_insert_with(|| InsightUserContext {
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

    async fn extract_memories_from_recent_conversation(
        db: &DatabaseConnection,
        memory: Option<Arc<dyn MemoryBackend>>,
        llm: Option<Arc<dyn LlmAdapter>>,
        user: &InsightUserContext,
        message_limit: u64,
        config: &InsightsSection,
    ) -> anyhow::Result<MemoryInsightStats> {
        let Some(memory) = memory else {
            return Ok(MemoryInsightStats {
                summary: "Extraction skipped: memory system is unavailable.".to_string(),
                model_used: "none".to_string(),
                ..MemoryInsightStats::default()
            });
        };

        let mut context = memory.write_snapshot(user.user_id).await?;
        let messages = Self::load_recent_messages(db, user, message_limit, &mut context).await?;
        if messages.is_empty() {
            return Ok(MemoryInsightStats {
                summary: "Extraction skipped: no recent conversation messages.".to_string(),
                model_used: memory.name().to_string(),
                ..MemoryInsightStats::default()
            });
        }

        let Some(llm) = llm else {
            return Ok(MemoryInsightStats {
                summary: "Extraction skipped: LLM is unavailable.".to_string(),
                model_used: memory.name().to_string(),
                ..MemoryInsightStats::default()
            });
        };
        let mut stats = MemoryInsightStats {
            model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
            ..MemoryInsightStats::default()
        };

        let transcript = transcript_for_prompt(&messages, 10_000);
        let response = llm
            .chat(&[
                ChatMessage::new("system", config.memory_extraction_prompt.clone()),
                ChatMessage::new(
                    "user",
                    format!(
                        "User id: {}\nRecent conversation transcript:\n{}",
                        user.user_id_text, transcript
                    ),
                ),
            ])
            .await?;
        let extracted = parse_memory_extraction_response(&response)?;
        stats.created += Self::store_memory_candidates(
            memory.as_ref(),
            user,
            &context,
            extracted.memories,
            "extraction",
        )
        .await?;
        stats.summary = format!(
            "Extraction stored {} selected memory candidates.",
            stats.created
        );
        Ok(stats)
    }

    async fn consolidate_memories(
        memory: Option<Arc<dyn MemoryBackend>>,
        llm: Option<Arc<dyn LlmAdapter>>,
        user: &InsightUserContext,
        limit: u32,
        config: &InsightsSection,
    ) -> anyhow::Result<MemoryInsightStats> {
        let Some(memory) = memory else {
            return Ok(MemoryInsightStats {
                summary: "Memory consolidation skipped: memory system is unavailable.".to_string(),
                model_used: "none".to_string(),
                ..MemoryInsightStats::default()
            });
        };
        let Some(llm) = llm else {
            return Ok(MemoryInsightStats {
                summary: "Memory consolidation skipped: LLM is unavailable.".to_string(),
                model_used: memory.name().to_string(),
                ..MemoryInsightStats::default()
            });
        };

        let mut context = memory.write_snapshot(user.user_id).await?;
        let memories = memory
            .list(
                user.user_id,
                MemoryListOpts {
                    max_results: Some(limit),
                    layers: None,
                },
            )
            .await?;
        context.source_memories = source_versions(&memories);
        if memories.is_empty() {
            return Ok(MemoryInsightStats {
                summary: "Memory consolidation skipped: no memories found.".to_string(),
                model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
                ..MemoryInsightStats::default()
            });
        }

        let response = llm
            .chat(&[
                ChatMessage::new("system", config.memory_consolidation_prompt.clone()),
                ChatMessage::new(
                    "user",
                    format!(
                        "User id: {}\nCurrent memories:\n{}",
                        user.user_id_text,
                        memories_for_prompt(&memories, 14_000)
                    ),
                ),
            ])
            .await?;
        let decision = parse_memory_consolidation_response(&response)?;
        let _proposed_summary = decision.summary;
        let mut batch = MemoryBatch::default();
        let mut selected = std::collections::HashSet::new();
        let updated = decision.updates.len() as u32;
        let retired = decision.retirements.len() as u32;
        for update in decision.updates {
            anyhow::ensure!(
                selected.insert(update.id.clone()),
                "duplicate consolidation target"
            );
            let source = memories
                .iter()
                .find(|m| m.id.as_deref() == Some(update.id.as_str()))
                .ok_or_else(|| anyhow::anyhow!("consolidation references an unknown memory"))?;
            if let Some(content) = &update.content {
                anyhow::ensure!(!content.trim().is_empty(), "empty consolidation content");
                anyhow::ensure!(
                    detect_sensitive_memory(content).is_none(),
                    "sensitive consolidation content rejected"
                );
            }
            anyhow::ensure!(
                update.content.is_some() || update.importance.is_some(),
                "empty consolidation update"
            );
            batch.updates.push((
                update.id,
                MemoryPatch {
                    expected_revision: Some(source.revision),
                    content: update.content.map(|content| content.trim().to_owned()),
                    importance: update.importance.map(clamp_score),
                    metadata: Some(serde_json::json!({
                        "source": "insights", "phase": "consolidation", "reason": update.reason,
                        "updated_at": Utc::now().to_rfc3339(),
                    })),
                    ..MemoryPatch::default()
                },
            ));
        }
        for retirement in decision.retirements {
            anyhow::ensure!(
                selected.insert(retirement.id.clone()),
                "duplicate consolidation target"
            );
            anyhow::ensure!(
                !retirement.reason.trim().is_empty(),
                "retirement requires a reason"
            );
            let source = memories
                .iter()
                .find(|m| m.id.as_deref() == Some(retirement.id.as_str()))
                .ok_or_else(|| anyhow::anyhow!("retirement references an unknown memory"))?;
            let status = match retirement.status {
                RetirementStatus::Superseded => "superseded",
                RetirementStatus::Rejected => "rejected",
            };
            batch.updates.push((
                retirement.id,
                MemoryPatch {
                    expected_revision: Some(source.revision),
                    status: Some(status.into()),
                    metadata: Some(serde_json::json!({
                        "source": "insights", "phase": "consolidation", "reason": retirement.reason,
                        "retired_at": Utc::now().to_rfc3339(),
                    })),
                    ..MemoryPatch::default()
                },
            ));
        }
        batch.creates = Self::prepare_memory_candidates(user, decision.creates, "consolidation")?;
        let committed = memory
            .apply_batch_guarded(&context, batch)
            .await
            .map_err(|error| anyhow::anyhow!("consolidation batch was not confirmed: {error}"))?;
        let created = committed.created_ids.len() as u32;
        anyhow::ensure!(
            committed.updated == (updated + retired) as usize,
            "memory batch receipt count mismatch"
        );
        Ok(MemoryInsightStats {
            created,
            updated,
            retired,
            pending: (memories.len() as u32).saturating_sub(updated + retired),
            model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
            summary: format!(
                "Consolidation committed {updated} updates, {retired} retirements and {created} creates atomically."
            ),
            ..MemoryInsightStats::default()
        })
    }

    async fn reflect_on_user_context(
        db: &DatabaseConnection,
        memory: Option<Arc<dyn MemoryBackend>>,
        llm: Option<Arc<dyn LlmAdapter>>,
        user: &InsightUserContext,
        memory_limit: u32,
        config: &InsightsSection,
    ) -> anyhow::Result<MemoryInsightStats> {
        let Some(memory) = memory else {
            return Ok(MemoryInsightStats {
                summary: "Reflection skipped: memory system is unavailable.".to_string(),
                model_used: "none".to_string(),
                ..MemoryInsightStats::default()
            });
        };
        let Some(llm) = llm else {
            return Ok(MemoryInsightStats {
                summary: "Reflection skipped: LLM is unavailable.".to_string(),
                model_used: memory.name().to_string(),
                ..MemoryInsightStats::default()
            });
        };

        let mut context = memory.write_snapshot(user.user_id).await?;
        let memories = memory
            .list(
                user.user_id,
                MemoryListOpts {
                    max_results: Some(memory_limit),
                    layers: None,
                },
            )
            .await?;
        context.source_memories = source_versions(&memories);
        let messages = Self::load_recent_messages(db, user, 16, &mut context).await?;

        if memories.is_empty() && messages.is_empty() {
            return Ok(MemoryInsightStats {
                summary: "Reflection skipped: no memory or conversation context found.".to_string(),
                model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
                ..MemoryInsightStats::default()
            });
        }

        let response = llm
            .chat(&[
                ChatMessage::new("system", config.reflection_prompt.clone()),
                ChatMessage::new(
                    "user",
                    format!(
                        "User id: {}\nCurrent memories:\n{}\n\nRecent conversation:\n{}",
                        user.user_id_text,
                        memories_for_prompt(&memories, 10_000),
                        transcript_for_prompt(&messages, 8_000)
                    ),
                ),
            ])
            .await?;
        let extracted = parse_memory_extraction_response(&response)?;
        let mut stats = MemoryInsightStats {
            model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
            ..MemoryInsightStats::default()
        };
        stats.created += Self::store_memory_candidates(
            memory.as_ref(),
            user,
            &context,
            extracted.memories,
            "reflection",
        )
        .await?;
        stats.summary = format!("Reflection stored {} durable insights.", stats.created);
        Ok(stats)
    }

    async fn detect_habits_from_context(
        db: &DatabaseConnection,
        memory: Option<Arc<dyn MemoryBackend>>,
        llm: Option<Arc<dyn LlmAdapter>>,
        user: &InsightUserContext,
        memory_limit: u32,
        config: &InsightsSection,
    ) -> anyhow::Result<MemoryInsightStats> {
        let Some(memory) = memory else {
            return Ok(MemoryInsightStats {
                summary: "Habit detection skipped: memory system is unavailable.".to_string(),
                model_used: "none".to_string(),
                ..MemoryInsightStats::default()
            });
        };
        let Some(llm) = llm else {
            return Ok(MemoryInsightStats {
                summary: "Habit detection skipped: LLM is unavailable.".to_string(),
                model_used: memory.name().to_string(),
                ..MemoryInsightStats::default()
            });
        };

        let mut context = memory.write_snapshot(user.user_id).await?;
        let memories = memory
            .list(
                user.user_id,
                MemoryListOpts {
                    max_results: Some(memory_limit),
                    layers: None,
                },
            )
            .await?;
        context.source_memories = source_versions(&memories);
        let messages = Self::load_recent_messages(db, user, 32, &mut context).await?;

        if memories.is_empty() && messages.is_empty() {
            return Ok(MemoryInsightStats {
                summary: "Habit detection skipped: no memory or conversation context found."
                    .to_string(),
                model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
                ..MemoryInsightStats::default()
            });
        }

        let response = llm
            .chat(&[
                ChatMessage::new("system", config.habit_detection_prompt.clone()),
                ChatMessage::new(
                    "user",
                    format!(
                        "User id: {}\nCurrent memories:\n{}\n\nRecent conversation:\n{}",
                        user.user_id_text,
                        memories_for_prompt(&memories, 10_000),
                        transcript_for_prompt(&messages, 10_000)
                    ),
                ),
            ])
            .await?;
        let extracted = parse_memory_extraction_response(&response)?;
        let mut stats = MemoryInsightStats {
            model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
            ..MemoryInsightStats::default()
        };
        stats.created += Self::store_memory_candidates(
            memory.as_ref(),
            user,
            &context,
            extracted.memories,
            "habit_detection",
        )
        .await?;
        stats.summary = format!("Habit detection stored {} durable habits.", stats.created);
        Ok(stats)
    }

    async fn store_memory_candidates(
        memory: &dyn MemoryBackend,
        user: &InsightUserContext,
        context: &MemoryWriteContext,
        candidates: Vec<MemoryCandidate>,
        phase: &str,
    ) -> anyhow::Result<u32> {
        let creates = Self::prepare_memory_candidates(user, candidates, phase)?;
        let result = memory
            .apply_batch_guarded(
                context,
                MemoryBatch {
                    creates,
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| anyhow::anyhow!("{phase} batch was not confirmed: {error}"))?;
        Ok(result.created_ids.len() as u32)
    }

    fn prepare_memory_candidates(
        user: &InsightUserContext,
        candidates: Vec<MemoryCandidate>,
        phase: &str,
    ) -> anyhow::Result<Vec<MemoryNode>> {
        anyhow::ensure!(candidates.len() <= 64, "too many insight memory candidates");
        let mut nodes = Vec::new();
        for candidate in candidates {
            let content = candidate.content.trim();
            let confidence = clamp_score(candidate.confidence.unwrap_or(1.0));
            if content.is_empty() || confidence < 0.6 || detect_sensitive_memory(content).is_some()
            {
                continue;
            }
            let layer = candidate
                .memory_layer
                .or(candidate.layer)
                .map(|layer| normalize_memory_layer(&layer))
                .unwrap_or_else(|| "core".into());
            let node_type = candidate
                .node_type
                .as_deref()
                .map(normalize_memory_node_type)
                .unwrap_or_else(|| "fact".into());
            let importance = clamp_score(candidate.importance.unwrap_or(0.6));
            let metadata = serde_json::json!({
                "source": "insights", "phase": phase, "confidence": confidence,
                "reason": candidate.reason, "user_id_text": user.user_id_text,
                "channel_id": user.channel_id, "created_at": Utc::now().to_rfc3339(),
            });
            nodes.push(
                MemoryNode::new(layer, node_type, content, importance)
                    .with_metadata(metadata)
                    .with_runtime_fields("semantic", "user", confidence, importance, "internal"),
            );
        }
        Ok(nodes)
    }

    async fn load_recent_messages(
        db: &DatabaseConnection,
        user: &InsightUserContext,
        limit: u64,
        context: &mut MemoryWriteContext,
    ) -> anyhow::Result<Vec<ChatMessage>> {
        let session_id = match user.session_id {
            Some(id) => Some(id),
            None => Self::find_latest_session_for_user(db, user).await?,
        };
        let Some(session_id) = session_id else {
            return Ok(Vec::new());
        };
        let mut rows = db.query_all(Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,
            "SELECT m.id,m.content,m.user_id FROM chat_messages m JOIN chat_sessions s ON s.id=m.session_id WHERE s.id=$1 AND s.creator_id=$2 AND m.user_id=$2 AND m.role='user' AND m.source_received_at IS NOT NULL AND m.source_privacy_epoch=$3 ORDER BY m.created_at DESC,m.id DESC LIMIT $4",
            vec![session_id.into(),user.user_id.to_string().into(),context.privacy_epoch.into(),(limit.clamp(1,100) as i64).into()])).await?;
        rows.reverse();
        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            context.source_event_ids.push(row.try_get("", "id")?);
            messages.push(ChatMessage {
                role: "user".into(),
                content: row.try_get("", "content")?,
                name: row.try_get("", "user_id")?,
                ..Default::default()
            });
        }
        Ok(messages)
    }

    async fn find_latest_session_for_user(
        db: &DatabaseConnection,
        user: &InsightUserContext,
    ) -> anyhow::Result<Option<Uuid>> {
        let mut condition = Condition::any()
            .add(chat_session::Column::CreatorId.eq(Some(user.user_id_text.clone())))
            .add(chat_session::Column::CreatorId.eq(Some(user.user_id.to_string())));
        if let Some(channel_id) = &user.channel_id {
            condition = condition.add(
                Condition::all()
                    .add(chat_session::Column::ChannelId.eq(channel_id.clone()))
                    .add(chat_session::Column::CreatorId.eq(Some(user.user_id_text.clone()))),
            );
        }
        Ok(chat_session::Entity::find()
            .filter(condition)
            .order_by_desc(chat_session::Column::UpdatedAt)
            .limit(1)
            .one(db)
            .await?
            .map(|session| session.id))
    }

    async fn review_pending_cache_entries(
        db: &DatabaseConnection,
        llm: Option<Arc<dyn LlmAdapter>>,
        user_id: Uuid,
        limit: usize,
    ) -> anyhow::Result<CacheReviewStats> {
        let entries = Self::load_pending_cache_entries(db, user_id, limit).await?;
        let mut stats = CacheReviewStats {
            pending: entries.len() as u32,
            model_used: "none".to_string(),
            ..CacheReviewStats::default()
        };

        let Some(llm) = llm else {
            if !entries.is_empty() {
                tracing::warn!(
                    pending = entries.len(),
                    "pending cache entries left disabled because no insights LLM is available"
                );
            }
            return Ok(stats);
        };

        stats.model_used = llm.name().to_string();
        for entry in entries {
            let decision = match Self::review_cache_entry_with_llm(llm.as_ref(), &entry).await {
                Ok(decision) => decision,
                Err(error) => {
                    tracing::warn!(
                        cache_entry_id = %entry.id,
                        error = ?error,
                        "cache review failed; leaving entry pending"
                    );
                    continue;
                }
            };

            let action = decision.decision.trim().to_ascii_lowercase();
            if action == "approve" {
                if Self::cache_review_approves_direct_return(&decision) {
                    let response_text = decision
                        .refined_response
                        .as_deref()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .unwrap_or(entry.response_text.trim())
                        .to_string();
                    Self::activate_cache_entry(db, &entry, response_text, &decision).await?;
                    stats.approved += 1;
                    stats.pending = stats.pending.saturating_sub(1);
                } else if Self::cache_review_has_explicit_unsafe_flag(&decision) {
                    Self::delete_cache_entry(db, entry.id, decision.reason.as_deref()).await?;
                    stats.deleted += 1;
                    stats.pending = stats.pending.saturating_sub(1);
                } else {
                    tracing::warn!(
                        cache_entry_id = %entry.id,
                        "cache review approved without all safety flags; leaving entry pending"
                    );
                }
            } else if matches!(action.as_str(), "delete" | "reject") {
                Self::delete_cache_entry(db, entry.id, decision.reason.as_deref()).await?;
                stats.deleted += 1;
                stats.pending = stats.pending.saturating_sub(1);
            } else {
                tracing::debug!(
                    cache_entry_id = %entry.id,
                    decision = %decision.decision,
                    reason = ?decision.reason,
                    "cache review left entry pending"
                );
            }
        }

        Ok(stats)
    }

    async fn load_pending_cache_entries(
        db: &DatabaseConnection,
        user_id: Uuid,
        limit: usize,
    ) -> anyhow::Result<Vec<PendingCacheEntry>> {
        let limit = limit.clamp(1, 100) as i64;
        let (sql, values): (&str, Vec<sea_orm::Value>) = if user_id.is_nil() {
            (
                r#"
                SELECT id::text AS id, query_text, response_text, metadata
                FROM llm_cache
                WHERE expires_at > NOW()
                    AND metadata->>'schema' = $1
                    AND metadata->>'status' = $2
                ORDER BY created_at ASC
                LIMIT $3
                "#,
                vec![
                    CACHE_METADATA_SCHEMA.into(),
                    CACHE_STATUS_PENDING.into(),
                    limit.into(),
                ],
            )
        } else {
            (
                r#"
                SELECT id::text AS id, query_text, response_text, metadata
                FROM llm_cache
                WHERE expires_at > NOW()
                    AND metadata->>'schema' = $1
                    AND metadata->>'status' = $2
                    AND user_id = $3::uuid
                ORDER BY created_at ASC
                LIMIT $4
                "#,
                vec![
                    CACHE_METADATA_SCHEMA.into(),
                    CACHE_STATUS_PENDING.into(),
                    user_id.to_string().into(),
                    limit.into(),
                ],
            )
        };

        let rows = db
            .query_all(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                sql,
                values,
            ))
            .await?;

        rows.into_iter()
            .map(|row| {
                let id: String = row.try_get("", "id")?;
                Ok(PendingCacheEntry {
                    id: Uuid::parse_str(&id)?,
                    query_text: row.try_get("", "query_text")?,
                    response_text: row.try_get("", "response_text")?,
                    metadata: row.try_get("", "metadata")?,
                })
            })
            .collect()
    }

    async fn review_cache_entry_with_llm(
        llm: &dyn LlmAdapter,
        entry: &PendingCacheEntry,
    ) -> anyhow::Result<CacheReviewDecision> {
        let messages = vec![
            ChatMessage::new(
                "system",
                "You are an internal cache reviewer. Decide whether a pending assistant response is safe to enable for direct exact/semantic cache reuse. Approve only stable, timeless, tool-free, action-free, memory-independent answers that can be returned to the same user without fresh LLM reasoning. Delete anything that depends on current time, live data, user memory, tools, device state, actions, private context, uncertainty, or incomplete/empty content. You may refine wording, but must not add new facts. Return exactly one JSON object with keys: decision, refined_response, reason, safe_direct_return, memory_independent, tool_free, dynamic_free. decision must be approve, delete, or keep_pending.",
            ),
            ChatMessage::new(
                "user",
                format!(
                    "Review this pending cache entry.\n\nUser query:\n{}\n\nCandidate assistant response:\n{}\n\nMetadata:\n{}",
                    entry.query_text,
                    entry.response_text,
                    serde_json::to_string_pretty(&entry.metadata).unwrap_or_default()
                ),
            ),
        ];

        let raw = llm.chat(&messages).await?;
        parse_cache_review_decision(&raw)
    }

    fn cache_review_approves_direct_return(decision: &CacheReviewDecision) -> bool {
        decision.safe_direct_return == Some(true)
            && decision.memory_independent == Some(true)
            && decision.tool_free == Some(true)
            && decision.dynamic_free == Some(true)
    }

    fn cache_review_has_explicit_unsafe_flag(decision: &CacheReviewDecision) -> bool {
        decision.safe_direct_return == Some(false)
            || decision.memory_independent == Some(false)
            || decision.tool_free == Some(false)
            || decision.dynamic_free == Some(false)
    }

    async fn activate_cache_entry(
        db: &DatabaseConnection,
        entry: &PendingCacheEntry,
        response_text: String,
        decision: &CacheReviewDecision,
    ) -> anyhow::Result<()> {
        let Some(model) = llm_cache::Entity::find_by_id(entry.id).one(db).await? else {
            return Ok(());
        };

        let mut metadata = entry.metadata.clone();
        if !metadata.is_object() {
            metadata = serde_json::json!({});
        }
        let object = metadata.as_object_mut().expect("metadata object");
        object.insert(
            "schema".to_string(),
            serde_json::Value::String(CACHE_METADATA_SCHEMA.to_string()),
        );
        object.insert(
            "status".to_string(),
            serde_json::Value::String(CACHE_STATUS_ACTIVE.to_string()),
        );
        object.insert("safe_direct_return".to_string(), serde_json::json!(true));
        object.insert("memory_independent".to_string(), serde_json::json!(true));
        object.insert("tool_free".to_string(), serde_json::json!(true));
        object.insert("dynamic_free".to_string(), serde_json::json!(true));
        object.insert(
            "reviewed_by".to_string(),
            serde_json::Value::String("insights".to_string()),
        );
        object.insert(
            "reviewed_at".to_string(),
            serde_json::Value::String(Utc::now().to_rfc3339()),
        );
        if let Some(reason) = decision
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            object.insert(
                "review_reason".to_string(),
                serde_json::Value::String(reason.to_string()),
            );
        }

        let mut active: llm_cache::ActiveModel = model.into();
        active.response_text = Set(response_text);
        active.metadata = Set(metadata);
        active.update(db).await?;
        Ok(())
    }

    async fn delete_cache_entry(
        db: &DatabaseConnection,
        entry_id: Uuid,
        reason: Option<&str>,
    ) -> anyhow::Result<()> {
        llm_cache::Entity::delete_by_id(entry_id).exec(db).await?;
        tracing::debug!(
            cache_entry_id = %entry_id,
            reason = ?reason,
            "deleted rejected pending cache entry"
        );
        Ok(())
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

impl Default for ScheduledInsights {
    fn default() -> Self {
        Self::new()
    }
}

impl InsightPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Extraction => "extraction",
            Self::Consolidation => "consolidation",
            Self::Reflection => "reflection",
            Self::HabitDetection => "habit_detection",
        }
    }
}

impl std::fmt::Display for InsightPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn parse_cache_review_decision(raw: &str) -> anyhow::Result<CacheReviewDecision> {
    parse_llm_json_object(raw, "cache review")
}

fn parse_memory_extraction_response(raw: &str) -> anyhow::Result<MemoryExtractionResponse> {
    parse_llm_json_object(raw, "memory extraction")
}

fn parse_memory_consolidation_response(raw: &str) -> anyhow::Result<MemoryConsolidationResponse> {
    parse_llm_json_object(raw, "memory consolidation")
}

fn parse_llm_json_object<T>(raw: &str, label: &str) -> anyhow::Result<T>
where
    T: DeserializeOwned,
{
    let candidate = strip_code_fences(raw);
    if let Ok(parsed) = serde_json::from_str::<T>(candidate) {
        return Ok(parsed);
    }

    let mut last_error = None;
    for json in json_object_slices(candidate) {
        match serde_json::from_str::<T>(json) {
            Ok(parsed) => return Ok(parsed),
            Err(error) => last_error = Some(error),
        }
    }

    if let Some(error) = last_error {
        anyhow::bail!("failed to parse {label} JSON: {error}");
    }
    anyhow::bail!("{label} response did not contain a JSON object")
}

fn strip_code_fences(s: &str) -> &str {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = if let Some(nl) = rest.find('\n') {
            &rest[nl + 1..]
        } else {
            rest
        };
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    }
}

fn json_object_slices(s: &str) -> Vec<&str> {
    let mut objects = Vec::new();
    let mut start = None;
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in s.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' if depth > 0 => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0
                    && let Some(start_index) = start.take()
                {
                    objects.push(&s[start_index..index + ch.len_utf8()]);
                }
            }
            _ => {}
        }
    }

    objects
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

fn transcript_for_prompt(messages: &[ChatMessage], max_chars: usize) -> String {
    let mut output = String::new();
    for message in messages {
        let role = message.role.trim();
        let name = message.name.as_deref().unwrap_or_default().trim();
        let content = message.content.trim();
        if content.is_empty() {
            continue;
        }
        if !name.is_empty() {
            output.push_str(&format!("{role}({name}): {content}\n"));
        } else {
            output.push_str(&format!("{role}: {content}\n"));
        }
    }
    truncate_for_prompt(&output, max_chars)
}

fn memories_for_prompt(memories: &[MemoryNode], max_chars: usize) -> String {
    let output = memories
        .iter()
        .enumerate()
        .map(|(index, memory)| {
            format!(
                "{}. id={} layer={} type={} importance={:.2}: {}",
                index + 1,
                memory.id.as_deref().unwrap_or(""),
                memory.memory_layer,
                memory.node_type,
                memory.importance,
                memory.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    truncate_for_prompt(&output, max_chars)
}

fn truncate_for_prompt(text: &str, max_chars: usize) -> String {
    let mut truncated = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        truncated.push_str("\n...");
    }
    truncated
}

fn clamp_score(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.5
    }
}

fn normalize_memory_layer(layer: &str) -> String {
    match layer.trim().to_ascii_lowercase().as_str() {
        "preference" | "preferences" => "preference".to_string(),
        "episode" | "episodic" | "event" => "episodic".to_string(),
        "reflection" | "insight" | "insights" => "reflection".to_string(),
        "core" | "profile" | "fact" => "core".to_string(),
        other if !other.is_empty() => other.to_string(),
        _ => "core".to_string(),
    }
}

fn normalize_memory_node_type(node_type: &str) -> String {
    match node_type.trim().to_ascii_lowercase().as_str() {
        "preference" | "fact" | "relationship" | "goal" | "habit" | "event" | "insight" => {
            node_type.trim().to_ascii_lowercase()
        }
        other if !other.is_empty() => other.to_string(),
        _ => "fact".to_string(),
    }
}

fn join_model_labels(labels: &[String]) -> String {
    let mut output = Vec::new();
    for label in labels {
        let label = label.trim();
        if !label.is_empty() && !output.iter().any(|existing: &&str| *existing == label) {
            output.push(label);
        }
    }
    if output.is_empty() {
        "none".to_string()
    } else {
        output.join("+")
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
impl ModuleLifecycle for ScheduledInsights {
    fn name(&self) -> &'static str {
        "scheduled"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let insights_config = ctx.config.policies.insights.clone();

        self.enabled
            .store(insights_config.enabled, Ordering::Relaxed);
        self.concurrency_semaphore = Arc::new(Semaphore::new(
            insights_config.max_concurrent_insights as usize,
        ));

        *self.config.lock().await = Some(insights_config);
        *self.db.lock().await = Some(ctx.db.clone());
        *self.llm.lock().await = ctx
            .major_llm
            .clone()
            .or_else(|| ctx.llm.clone())
            .or_else(|| ctx.minor_llm.clone());
        *self.memory.lock().await = ctx.memory.clone();
        *self.event_bus.lock().await = Some(ctx.event_bus.clone());
        *self.cancel_token.lock().await = Some(ctx.cancel_token.clone());
        *self.scheduler.lock().await = ctx.scheduler.clone();
        *self.condition_registry.lock().await = ctx.conditions.clone();

        tracing::info!("ScheduledInsights initialized");
        Ok(())
    }

    async fn start(&self) -> anyhow::Result<()> {
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
        background.spawn_loop("insights events", async move {
            Self::event_listener_loop(
                db_clone,
                eb_clone,
                listener_llm,
                listener_memory,
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
        tracing::info!("ScheduledInsights started (2 background tasks, cron via scheduler)");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
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
        tracing::info!("ScheduledInsights stopped");
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if !self.enabled.load(Ordering::Relaxed) {
            return ModuleHealth::Degraded("insights disabled".to_string());
        }
        ModuleHealth::Healthy
    }

    async fn apply_settings(
        &self,
        updates: &std::collections::BTreeMap<String, serde_json::Value>,
    ) -> anyhow::Result<Vec<String>> {
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

#[async_trait]
impl InsightStrategy for ScheduledInsights {
    async fn schedule(&self, task: InsightTask) -> anyhow::Result<()> {
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

    async fn status(&self, user_id: Uuid) -> anyhow::Result<InsightStatus> {
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

    async fn trigger(&self, user_id: Uuid, phase: InsightPhase) -> anyhow::Result<()> {
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
        let user = Self::resolve_phase_users(&db, &self.state, Some(user_id))
            .await
            .into_iter()
            .next()
            .unwrap_or_else(|| InsightUserContext {
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
mod tests {
    use super::*;
    use crate::providers::memory::AliaMemory;

    struct FixedMemoryLlm {
        response: String,
        calls: AtomicU32,
    }

    #[async_trait]
    impl ModuleLifecycle for FixedMemoryLlm {
        fn name(&self) -> &'static str {
            "fixed_insights_memory_llm"
        }
    }

    #[async_trait]
    impl LlmAdapter for FixedMemoryLlm {
        async fn chat(&self, _messages: &[ChatMessage]) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.response.clone())
        }

        async fn chat_stream(
            &self,
            _messages: &[ChatMessage],
        ) -> anyhow::Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<String>> + Send>>,
        > {
            anyhow::bail!("insights memory tests require the non-streaming call path")
        }
    }

    fn insight_user() -> InsightUserContext {
        let user_id = Uuid::new_v4();
        InsightUserContext {
            user_id,
            user_id_text: user_id.to_string(),
            channel_id: Some("insights-test".into()),
            session_id: None,
            last_seen_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn consolidation_commits_multiple_updates_and_creates_in_one_snapshot() {
        let memory = Arc::new(AliaMemory::new());
        let user = insight_user();
        let first = memory
            .store(
                user.user_id,
                MemoryNode::new("core", "fact", "First original fact", 0.8),
            )
            .await
            .unwrap();
        let second = memory
            .store(
                user.user_id,
                MemoryNode::new("core", "fact", "Second original fact", 0.8),
            )
            .await
            .unwrap();
        let original_epoch = memory
            .write_snapshot(user.user_id)
            .await
            .unwrap()
            .privacy_epoch;
        let llm = Arc::new(FixedMemoryLlm {
            response: serde_json::json!({
                "summary": "Everything has been consolidated successfully",
                "updates": [
                    {"id": first, "content": "First revised fact"},
                    {"id": second, "content": "Second revised fact"}
                ],
                "retirements": [],
                "creates": [{"content": "A durable combined fact"}]
            })
            .to_string(),
            calls: AtomicU32::new(0),
        });

        let stats = ScheduledInsights::consolidate_memories(
            Some(memory.clone()),
            Some(llm.clone()),
            &user,
            50,
            &InsightsSection::default(),
        )
        .await
        .unwrap();
        assert_eq!((stats.updated, stats.retired, stats.created), (2, 0, 1));
        assert!(stats.summary.contains("atomically"));
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);

        let stored_first = memory.get(user.user_id, &first).await.unwrap().unwrap();
        let stored_second = memory.get(user.user_id, &second).await.unwrap().unwrap();
        assert_eq!(stored_first.content, "First revised fact");
        assert_eq!(stored_first.revision, 2);
        assert_eq!(stored_second.content, "Second revised fact");
        assert_eq!(stored_second.revision, 2);
        assert_eq!(
            memory
                .list(user.user_id, MemoryListOpts::default())
                .await
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            memory
                .write_snapshot(user.user_id)
                .await
                .unwrap()
                .privacy_epoch,
            original_epoch
        );
    }

    #[tokio::test]
    async fn candidate_write_after_clear_returns_error_instead_of_zero_created_success() {
        let memory = AliaMemory::new();
        let user = insight_user();
        let context = memory.write_snapshot(user.user_id).await.unwrap();
        memory.clear(user.user_id).await.unwrap();
        let cleared_epoch = memory
            .write_snapshot(user.user_id)
            .await
            .unwrap()
            .privacy_epoch;
        assert!(cleared_epoch > context.privacy_epoch);
        let candidates = parse_memory_extraction_response(
            r#"{"memories":[{"content":"Old source says the user prefers concise explanations","confidence":0.95}]}"#,
        ).unwrap().memories;

        let error = ScheduledInsights::store_memory_candidates(
            &memory,
            &user,
            &context,
            candidates,
            "extraction",
        )
        .await
        .expect_err("a rejected old candidate must not be swallowed as Ok(0)");
        let message = error.to_string();
        assert!(
            message.contains("extraction batch was not confirmed"),
            "{message}"
        );
        assert!(
            message.contains("memory privacy epoch changed"),
            "{message}"
        );
        assert!(
            memory
                .list(user.user_id, MemoryListOpts::default())
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            memory
                .write_snapshot(user.user_id)
                .await
                .unwrap()
                .privacy_epoch,
            cleared_epoch
        );
    }

    #[tokio::test]
    async fn invalid_later_consolidation_target_leaves_the_entire_plan_unwritten() {
        let memory = Arc::new(AliaMemory::new());
        let user = insight_user();
        let id = memory
            .store(
                user.user_id,
                MemoryNode::new("core", "fact", "Original fact", 0.8),
            )
            .await
            .unwrap();
        let llm = Arc::new(FixedMemoryLlm {
            response: serde_json::json!({
                "updates": [{"id": id, "content": "Revised fact"}],
                "retirements": [{"id": Uuid::new_v4(), "status": "superseded", "reason": "duplicate"}],
                "creates": [{"content": "New fact"}]
            }).to_string(), calls: AtomicU32::new(0),
        });
        assert!(
            ScheduledInsights::consolidate_memories(
                Some(memory.clone()),
                Some(llm),
                &user,
                50,
                &InsightsSection::default(),
            )
            .await
            .is_err()
        );
        let unchanged = memory.get(user.user_id, &id).await.unwrap().unwrap();
        assert_eq!(
            (unchanged.content.as_str(), unchanged.revision),
            ("Original fact", 1)
        );
        assert_eq!(
            memory
                .list(user.user_id, MemoryListOpts::default())
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn consolidation_retirement_preserves_evidence_and_does_not_advance_privacy_epoch() {
        let memory = Arc::new(AliaMemory::new());
        let user = insight_user();
        let id = memory
            .store(
                user.user_id,
                MemoryNode::new("core", "fact", "Old wording", 0.8),
            )
            .await
            .unwrap();
        let epoch = memory
            .write_snapshot(user.user_id)
            .await
            .unwrap()
            .privacy_epoch;
        let llm = Arc::new(FixedMemoryLlm {
            response: serde_json::json!({
                "retirements": [{"id": id, "status": "superseded", "reason": "refined wording"}],
                "creates": [{"content": "Refined wording"}]
            })
            .to_string(),
            calls: AtomicU32::new(0),
        });
        let stats = ScheduledInsights::consolidate_memories(
            Some(memory.clone()),
            Some(llm),
            &user,
            50,
            &InsightsSection::default(),
        )
        .await
        .unwrap();
        assert_eq!((stats.retired, stats.created), (1, 1));
        let retired = memory.get(user.user_id, &id).await.unwrap().unwrap();
        assert_eq!(retired.status, "superseded");
        assert_eq!(retired.content, "Old wording");
        assert_eq!(
            memory
                .write_snapshot(user.user_id)
                .await
                .unwrap()
                .privacy_epoch,
            epoch
        );
        let mut forget = memory.write_snapshot(user.user_id).await.unwrap();
        forget.source_memories = vec![(id, retired.revision)];
        assert_eq!(
            memory
                .forget_many_guarded(&forget, &forget.source_memories)
                .await
                .unwrap(),
            2
        );
        assert!(
            memory
                .list(user.user_id, MemoryListOpts::default())
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parses_fenced_cache_review_decision() {
        let decision = parse_cache_review_decision(
            "```json\n{\"decision\":\"approve\",\"refined_response\":\"Stable answer.\",\"safe_direct_return\":true,\"memory_independent\":true,\"tool_free\":true,\"dynamic_free\":true}\n```",
        )
        .unwrap();

        assert_eq!(decision.decision, "approve");
        assert!(ScheduledInsights::cache_review_approves_direct_return(
            &decision
        ));
    }

    #[test]
    fn parses_cache_review_with_surrounding_text() {
        let decision = parse_cache_review_decision(
            "Here is the review:\n{\"decision\":\"delete\",\"reason\":\"live data\",\"safe_direct_return\":false,\"memory_independent\":true,\"tool_free\":true,\"dynamic_free\":false}",
        )
        .unwrap();

        assert_eq!(decision.decision, "delete");
        assert!(ScheduledInsights::cache_review_has_explicit_unsafe_flag(
            &decision
        ));
    }

    #[test]
    fn parses_memory_extraction_with_fence_and_reason() {
        let parsed = parse_memory_extraction_response(
            "```json\n{\"memories\":[{\"content\":\"KeQiao likes tsukuyomi\",\"memory_layer\":\"preference\",\"node_type\":\"preference\",\"importance\":0.9,\"confidence\":0.95,\"reason\":\"explicitly stated\"}]}\n```",
        )
        .unwrap();

        assert_eq!(parsed.memories.len(), 1);
        assert_eq!(parsed.memories[0].content, "KeQiao likes tsukuyomi");
        assert_eq!(
            parsed.memories[0].memory_layer.as_deref(),
            Some("preference")
        );
    }

    #[test]
    fn parses_memory_consolidation_with_surrounding_text() {
        let parsed = parse_memory_consolidation_response(
            "Result:\n{\"summary\":\"merged duplicates\",\"updates\":[{\"id\":\"mem_1\",\"content\":\"KeQiao likes Japanese music\",\"importance\":0.8,\"reason\":\"clearer\"}],\"retirements\":[{\"id\":\"mem_2\",\"status\":\"superseded\",\"reason\":\"duplicate\"}],\"creates\":[]}\nThanks.",
        )
        .unwrap();

        assert_eq!(parsed.summary.as_deref(), Some("merged duplicates"));
        assert_eq!(parsed.updates[0].id, "mem_1");
        assert_eq!(parsed.retirements[0].id, "mem_2");
    }

    #[test]
    fn json_object_slices_handles_braces_inside_strings() {
        let objects = json_object_slices(r#"prefix {"value":"literal { brace }"} suffix"#);

        assert_eq!(objects, vec![r#"{"value":"literal { brace }"}"#]);
    }

    #[test]
    fn stable_user_uuid_never_returns_nil_for_empty_text() {
        assert!(!stable_user_uuid("").is_nil());
        assert_eq!(
            stable_user_uuid("sample-user"),
            stable_user_uuid("sample-user")
        );
    }
}

fn source_versions(memories: &[MemoryNode]) -> Vec<(String, i64)> {
    memories
        .iter()
        .filter_map(|memory| memory.id.clone().map(|id| (id, memory.revision)))
        .collect()
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::providers::adapter::TaskFilter;
    use crate::scheduler::action::ActionRegistry;
    use crate::scheduler::{AutomationScheduler, ConditionRegistry};

    #[tokio::test]
    async fn failed_second_cron_registration_rolls_back_first_and_rejects_restart() {
        let scheduler = Arc::new(AutomationScheduler::with_registries(
            Arc::new(ConditionRegistry::new()),
            Arc::new(ActionRegistry::new()),
        ));
        let insights = ScheduledInsights::new();
        let mut config = InsightsSection::default();
        config.enabled = true;
        config.habit_detection_schedule = "invalid cron".into();
        *insights.config.lock().await = Some(config);
        *insights.db.lock().await = Some(DatabaseConnection::Disconnected);
        *insights.event_bus.lock().await = Some(EventBus::new(8));
        *insights.cancel_token.lock().await = Some(CancellationToken::new());
        *insights.scheduler.lock().await = Some(scheduler.clone());
        assert!(insights.start().await.is_err());
        assert_eq!(
            scheduler
                .list_tasks(TaskFilter::default())
                .await
                .unwrap()
                .len(),
            1
        );
        insights.stop().await.unwrap();
        assert!(
            scheduler
                .list_tasks(TaskFilter::default())
                .await
                .unwrap()
                .is_empty()
        );
        insights.stop().await.unwrap();
        assert!(insights.start().await.is_err());
        assert!(insights.set_conditions(vec![]).await.is_err());
    }
}
