use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;

#[derive(Serialize)]
pub struct ProcessInfo {
    pub name: String,
    pub state: String,
    pub pid: Option<u32>,
}

async fn list_processes(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<ProcessInfo>>> {
    // TODO: forward to gRPC ListProcesses
    Ok(Json(Vec::new()))
}

async fn start_process(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    // TODO: forward to gRPC StartProcess
    Ok(Json(serde_json::json!({"success": true})))
}

async fn stop_process(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    // TODO: forward to gRPC StopProcess
    Ok(Json(serde_json::json!({"success": true})))
}

async fn restart_process(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    // TODO: forward to gRPC RestartProcess
    Ok(Json(serde_json::json!({"success": true})))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/processes", get(list_processes))
        .route("/processes/{name}/start", post(start_process))
        .route("/processes/{name}/stop", post(stop_process))
        .route("/processes/{name}/restart", post(restart_process))
}
