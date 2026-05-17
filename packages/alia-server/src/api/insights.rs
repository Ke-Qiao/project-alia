use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::entity::insight_log;
use crate::error::Result;
use crate::providers::adapter::InsightPhase;
use crate::state::AppState;

#[derive(Serialize)]
pub struct InsightStatusResponse {
    pub enabled: bool,
    pub last_phase: Option<String>,
    pub last_executed_at: Option<String>,
    pub pending_tasks: u32,
}

#[derive(Serialize)]
pub struct InsightLogResponse {
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
) -> Result<Json<InsightStatusResponse>> {
    let status = state
        .modules
        .insights
        .status(auth.id)
        .await
        .map_err(crate::error::AppError::Internal)?;

    Ok(Json(InsightStatusResponse {
        enabled: status.enabled,
        last_phase: status.last_phase,
        last_executed_at: status.last_executed_at.map(|t| t.to_rfc3339()),
        pending_tasks: status.pending_tasks,
    }))
}

async fn toggle(State(state): State<AppState>, _auth: AuthUser) -> Result<Json<serde_json::Value>> {
    let config_enabled = state.config.insights.enabled;
    Ok(Json(serde_json::json!({
        "enabled": config_enabled,
        "message": "insights toggle is config-driven; update server.toml [insights].enabled"
    })))
}

async fn get_logs(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<InsightLogResponse>>> {
    let paginator = insight_log::Entity::find()
        .filter(insight_log::Column::UserId.eq(auth.id))
        .order_by_desc(insight_log::Column::CreatedAt)
        .paginate(&state.db, params.per_page);

    let total = paginator
        .num_items()
        .await
        .map_err(crate::error::AppError::Database)?;

    let page_idx = params.page.saturating_sub(1);
    let logs = paginator
        .fetch_page(page_idx)
        .await
        .map_err(crate::error::AppError::Database)?;

    Ok(Json(PaginatedResponse {
        data: logs.into_iter().map(log_response).collect(),
        total,
        page: params.page,
        per_page: params.per_page,
    }))
}

async fn get_reflections(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<InsightLogResponse>>> {
    let logs = insight_log::Entity::find()
        .filter(insight_log::Column::UserId.eq(auth.id))
        .filter(insight_log::Column::Phase.eq("reflection"))
        .order_by_desc(insight_log::Column::CreatedAt)
        .all(&state.db)
        .await
        .map_err(crate::error::AppError::Database)?;

    Ok(Json(logs.into_iter().map(log_response).collect()))
}

async fn trigger_insights(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<TriggerRequest>,
) -> Result<Json<serde_json::Value>> {
    let phase = match body.phase.as_str() {
        "extraction" => InsightPhase::Extraction,
        "consolidation" => InsightPhase::Consolidation,
        "reflection" => InsightPhase::Reflection,
        "habit_detection" => InsightPhase::HabitDetection,
        other => {
            return Err(crate::error::AppError::Validation(format!(
                "unknown phase: {other}; valid: extraction, consolidation, reflection, habit_detection"
            )));
        }
    };

    state
        .modules
        .insights
        .trigger(auth.id, phase)
        .await
        .map_err(crate::error::AppError::Internal)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "phase": body.phase,
    })))
}

async fn get_conditions(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>> {
    Ok(Json(state.modules.insights.get_conditions().await))
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
        .insights
        .set_conditions(body.conditions.clone())
        .await
        .map_err(crate::error::AppError::Internal)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "conditions_count": body.conditions.len(),
    })))
}

fn log_response(log: insight_log::Model) -> InsightLogResponse {
    InsightLogResponse {
        id: log.id,
        phase: log.phase,
        trigger_type: log.trigger_type,
        summary: log.summary,
        nodes_created: log.nodes_created,
        nodes_updated: log.nodes_updated,
        nodes_merged: log.nodes_merged,
        edges_created: log.edges_created,
        model_used: log.model_used,
        duration_ms: log.duration_ms,
        created_at: log.created_at.to_rfc3339(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/insights/status", get(get_status))
        .route("/insights/toggle", post(toggle))
        .route("/insights/logs", get(get_logs))
        .route("/insights/reflections", get(get_reflections))
        .route("/insights/trigger", post(trigger_insights))
        .route(
            "/insights/conditions",
            get(get_conditions).put(set_conditions),
        )
}
