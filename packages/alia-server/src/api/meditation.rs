use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::entity::meditation_log;
use crate::error::Result;
use crate::providers::adapter::MeditationPhase;
use crate::state::AppState;

use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder};

#[derive(Serialize)]
pub struct MeditationStatusResponse {
    pub enabled: bool,
    pub last_phase: Option<String>,
    pub last_executed_at: Option<String>,
    pub pending_tasks: u32,
}

#[derive(Serialize)]
pub struct MeditationLogResponse {
    pub id: Uuid,
    pub phase: String,
    pub trigger_type: String,
    pub summary: String,
    pub nodes_created: i32,
    pub nodes_updated: i32,
    pub nodes_merged: i32,
    pub edges_created: i32,
    pub model_used: Option<String>,
    pub duration_ms: Option<i32>,
    pub created_at: String,
}

#[derive(Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_page")]
    pub page: u64,
    #[serde(default = "default_per_page")]
    pub per_page: u64,
}

fn default_page() -> u64 {
    1
}
fn default_per_page() -> u64 {
    20
}

#[derive(Serialize)]
pub struct PaginatedResponse<T: Serialize> {
    pub data: Vec<T>,
    pub total: u64,
    pub page: u64,
    pub per_page: u64,
}

#[derive(Deserialize)]
pub struct TriggerRequest {
    #[serde(default = "default_phase")]
    pub phase: String,
}

fn default_phase() -> String {
    "extraction".to_string()
}

async fn get_status(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<MeditationStatusResponse>> {
    let status = state
        .modules
        .meditation
        .status(auth.id)
        .await
        .map_err(|e| crate::error::AppError::Internal(e))?;

    Ok(Json(MeditationStatusResponse {
        enabled: status.enabled,
        last_phase: status.last_phase,
        last_executed_at: status.last_executed_at.map(|t| t.to_rfc3339()),
        pending_tasks: status.pending_tasks,
    }))
}

async fn toggle(State(state): State<AppState>, _auth: AuthUser) -> Result<Json<serde_json::Value>> {
    // Toggle is a config-level operation; for now we report current state
    let config_enabled = state.config.meditation.enabled;
    Ok(Json(serde_json::json!({
        "enabled": config_enabled,
        "message": "meditation toggle is config-driven; update server.toml [meditation].enabled"
    })))
}

async fn get_logs(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<MeditationLogResponse>>> {
    let paginator = meditation_log::Entity::find()
        .filter(meditation_log::Column::UserId.eq(auth.id))
        .order_by_desc(meditation_log::Column::CreatedAt)
        .paginate(&state.db, params.per_page);

    let total = paginator
        .num_items()
        .await
        .map_err(|e| crate::error::AppError::Database(e))?;

    let page_idx = if params.page > 0 { params.page - 1 } else { 0 };
    let logs = paginator
        .fetch_page(page_idx)
        .await
        .map_err(|e| crate::error::AppError::Database(e))?;

    let data: Vec<MeditationLogResponse> = logs
        .into_iter()
        .map(|l| MeditationLogResponse {
            id: l.id,
            phase: l.phase,
            trigger_type: l.trigger_type,
            summary: l.summary,
            nodes_created: l.nodes_created,
            nodes_updated: l.nodes_updated,
            nodes_merged: l.nodes_merged,
            edges_created: l.edges_created,
            model_used: l.model_used,
            duration_ms: l.duration_ms,
            created_at: l.created_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(PaginatedResponse {
        data,
        total,
        page: params.page,
        per_page: params.per_page,
    }))
}

async fn get_insights(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<MeditationLogResponse>>> {
    // Insights are reflection-phase logs
    let logs = meditation_log::Entity::find()
        .filter(meditation_log::Column::UserId.eq(auth.id))
        .filter(meditation_log::Column::Phase.eq("reflection"))
        .order_by_desc(meditation_log::Column::CreatedAt)
        .all(&state.db)
        .await
        .map_err(|e| crate::error::AppError::Database(e))?;

    let data: Vec<MeditationLogResponse> = logs
        .into_iter()
        .map(|l| MeditationLogResponse {
            id: l.id,
            phase: l.phase,
            trigger_type: l.trigger_type,
            summary: l.summary,
            nodes_created: l.nodes_created,
            nodes_updated: l.nodes_updated,
            nodes_merged: l.nodes_merged,
            edges_created: l.edges_created,
            model_used: l.model_used,
            duration_ms: l.duration_ms,
            created_at: l.created_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(data))
}

async fn trigger_meditation(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<TriggerRequest>,
) -> Result<Json<serde_json::Value>> {
    let phase = match body.phase.as_str() {
        "extraction" => MeditationPhase::Extraction,
        "consolidation" => MeditationPhase::Consolidation,
        "reflection" => MeditationPhase::Reflection,
        "habit_detection" => MeditationPhase::HabitDetection,
        other => {
            return Err(crate::error::AppError::Validation(format!(
                "unknown phase: {other}; valid: extraction, consolidation, reflection, habit_detection"
            )));
        }
    };

    state
        .modules
        .meditation
        .trigger(auth.id, phase)
        .await
        .map_err(|e| crate::error::AppError::Internal(e))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "phase": body.phase,
    })))
}

async fn get_conditions(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>> {
    let conditions = state.modules.meditation.get_conditions().await;
    Ok(Json(conditions))
}

#[derive(Deserialize)]
pub struct SetConditionsRequest {
    pub conditions: Vec<serde_json::Value>,
}

async fn set_conditions(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<SetConditionsRequest>,
) -> Result<Json<serde_json::Value>> {
    state
        .modules
        .meditation
        .set_conditions(body.conditions.clone())
        .await
        .map_err(|e| crate::error::AppError::Internal(e))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "conditions_count": body.conditions.len(),
    })))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/meditation/status", get(get_status))
        .route("/meditation/toggle", post(toggle))
        .route("/meditation/logs", get(get_logs))
        .route("/meditation/insights", get(get_insights))
        .route("/meditation/trigger", post(trigger_meditation))
        .route(
            "/meditation/conditions",
            get(get_conditions).put(set_conditions),
        )
}
