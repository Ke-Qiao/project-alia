use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, Result};
use crate::providers::adapter::{
    EventSubscription, ScheduleConfig, ScheduledTask, TaskFilter, TaskPatch,
};
use crate::state::AppState;

// ========== Request/Response Types ==========

#[derive(Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub name: String,
    pub schedule: ScheduleConfig,
    pub conditions: serde_json::Value,
    pub action: serde_json::Value,
    #[serde(default = "default_cooldown")]
    pub cooldown_sec: u32,
    #[serde(default)]
    pub max_retries: u32,
}

fn default_cooldown() -> u32 {
    0
}

#[derive(Serialize, Deserialize)]
pub struct UpdateTaskRequest {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub schedule: Option<ScheduleConfig>,
    pub conditions: Option<serde_json::Value>,
    pub action: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
pub struct CreateSubscriptionRequest {
    pub name: String,
    pub event_pattern: serde_json::Value,
    pub conditions: serde_json::Value,
    pub action: serde_json::Value,
    #[serde(default = "default_cooldown")]
    pub cooldown_sec: u32,
}

// ========== Handlers ==========

/// List all scheduled tasks for the current user
async fn list_tasks(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<ScheduledTask>>> {
    let filter = TaskFilter {
        user_id: Some(auth.id),
        enabled_only: false,
    };
    let tasks = state.modules.scheduler.list_tasks(filter).await?;
    Ok(Json(tasks))
}

/// Create a new scheduled task
async fn create_task(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<CreateTaskRequest>,
) -> Result<Json<ScheduledTask>> {
    let task = ScheduledTask {
        id: Uuid::new_v4(),
        user_id: auth.id,
        name: body.name,
        enabled: true,
        schedule: body.schedule,
        conditions: body.conditions,
        action: body.action,
        cooldown_sec: body.cooldown_sec,
        max_retries: body.max_retries,
        created_by: "user".to_string(),
    };

    let task_id = state.modules.scheduler.create_task(task.clone()).await?;
    tracing::info!(%task_id, "task created");

    Ok(Json(task))
}

/// Get a specific task by ID
async fn get_task(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ScheduledTask>> {
    let filter = TaskFilter {
        user_id: Some(auth.id),
        enabled_only: false,
    };
    let tasks = state.modules.scheduler.list_tasks(filter).await?;

    tasks
        .into_iter()
        .find(|t| t.id == id)
        .map(Json)
        .ok_or_else(|| crate::error::AppError::NotFound("Task not found".into()))
}

/// Update a task
async fn update_task(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTaskRequest>,
) -> Result<Json<serde_json::Value>> {
    ensure_task_owner(&state, &auth, id).await?;
    let patch = TaskPatch {
        name: body.name,
        enabled: body.enabled,
        schedule: body.schedule,
        conditions: body.conditions,
        action: body.action,
    };

    state.modules.scheduler.update_task(id, patch).await?;
    Ok(Json(serde_json::json!({"success": true, "task_id": id})))
}

/// Delete a task
async fn delete_task_handler(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    ensure_task_owner(&state, &auth, id).await?;
    state.modules.scheduler.delete_task(id).await?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// Pause a task
async fn pause_task(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    ensure_task_owner(&state, &auth, id).await?;
    state.modules.scheduler.pause_task(id).await?;
    Ok(Json(serde_json::json!({"success": true, "paused": true})))
}

/// Resume a task
async fn resume_task(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    ensure_task_owner(&state, &auth, id).await?;
    state.modules.scheduler.resume_task(id).await?;
    Ok(Json(serde_json::json!({"success": true, "paused": false})))
}

// ========== Event Subscriptions ==========

/// List all event subscriptions for the current user
async fn list_subscriptions(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<EventSubscription>>> {
    let subs = state.modules.scheduler.list_subscriptions(auth.id).await?;
    Ok(Json(subs))
}

/// Create a new event subscription
async fn create_subscription(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<CreateSubscriptionRequest>,
) -> Result<Json<EventSubscription>> {
    let sub = EventSubscription {
        id: Uuid::new_v4(),
        user_id: auth.id,
        name: body.name,
        enabled: true,
        event_pattern: body.event_pattern,
        conditions: body.conditions,
        action: body.action,
        cooldown_sec: body.cooldown_sec,
        created_by: "user".to_string(),
    };

    let sub_id = state.modules.scheduler.subscribe_event(sub.clone()).await?;
    tracing::info!(%sub_id, "subscription created");

    Ok(Json(sub))
}

/// Delete an event subscription
async fn delete_subscription(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    ensure_subscription_owner(&state, &auth, id).await?;
    state.modules.scheduler.unsubscribe_event(id).await?;
    Ok(Json(serde_json::json!({"success": true})))
}

async fn ensure_task_owner(state: &AppState, auth: &AuthUser, id: Uuid) -> Result<()> {
    let filter = TaskFilter {
        user_id: Some(auth.id),
        enabled_only: false,
    };
    let owned = state
        .modules
        .scheduler
        .list_tasks(filter)
        .await?
        .into_iter()
        .any(|task| task.id == id);
    if owned {
        Ok(())
    } else {
        Err(AppError::NotFound("Task not found".into()))
    }
}

async fn ensure_subscription_owner(state: &AppState, auth: &AuthUser, id: Uuid) -> Result<()> {
    let owned = state
        .modules
        .scheduler
        .list_subscriptions(auth.id)
        .await?
        .into_iter()
        .any(|sub| sub.id == id);
    if owned {
        Ok(())
    } else {
        Err(AppError::NotFound("Subscription not found".into()))
    }
}

// ========== Routes ==========

pub fn routes() -> Router<AppState> {
    Router::new()
        // Tasks
        .route("/scheduler/tasks", get(list_tasks).post(create_task))
        .route(
            "/scheduler/tasks/{id}",
            get(get_task).put(update_task).delete(delete_task_handler),
        )
        .route("/scheduler/tasks/{id}/pause", post(pause_task))
        .route("/scheduler/tasks/{id}/resume", post(resume_task))
        // Subscriptions
        .route(
            "/scheduler/subscriptions",
            get(list_subscriptions).post(create_subscription),
        )
        .route("/scheduler/subscriptions/{id}", delete(delete_subscription))
}
