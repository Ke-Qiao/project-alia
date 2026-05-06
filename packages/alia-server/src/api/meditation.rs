use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;

#[derive(Serialize)]
pub struct MeditationStatusResponse {
    pub enabled: bool,
    pub last_phase: Option<String>,
    pub last_executed_at: Option<String>,
}

#[derive(Serialize)]
pub struct MeditationLog {
    pub phase: String,
    pub trigger_type: String,
    pub summary: String,
    pub created_at: String,
}

async fn get_status(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<MeditationStatusResponse>> {
    Ok(Json(MeditationStatusResponse {
        enabled: false,
        last_phase: None,
        last_executed_at: None,
    }))
}

async fn toggle(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({"success": true})))
}

async fn get_logs(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<MeditationLog>>> {
    Ok(Json(Vec::new()))
}

async fn get_insights(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>> {
    Ok(Json(Vec::new()))
}

async fn trigger_meditation(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({"success": true})))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/meditation/status", get(get_status))
        .route("/meditation/toggle", post(toggle))
        .route("/meditation/logs", get(get_logs))
        .route("/meditation/insights", get(get_insights))
        .route("/meditation/trigger", post(trigger_meditation))
}
