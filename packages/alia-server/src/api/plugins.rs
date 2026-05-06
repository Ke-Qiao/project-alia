use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;

#[derive(Serialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub plugin_type: String,
    pub state: String,
}

async fn list_plugins(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Vec<PluginInfo>>> {
    // TODO: forward to gRPC ListPlugins
    Ok(Json(Vec::new()))
}

async fn install_plugin(
    State(_state): State<AppState>,
    _auth: AuthUser,
    _body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>> {
    // TODO: forward to gRPC InstallPlugin
    Ok(Json(serde_json::json!({"success": true})))
}

async fn uninstall_plugin(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    // TODO: forward to gRPC UninstallPlugin
    Ok(Json(serde_json::json!({"success": true})))
}

async fn start_plugin(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    // TODO: forward to gRPC StartPlugin
    Ok(Json(serde_json::json!({"success": true})))
}

async fn stop_plugin(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    // TODO: forward to gRPC StopPlugin
    Ok(Json(serde_json::json!({"success": true})))
}

async fn send_command(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Path(_name): Path<String>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>> {
    // TODO: forward to gRPC SendCommand
    Ok(Json(serde_json::json!({"success": true})))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/plugins", get(list_plugins))
        .route("/plugins/install", post(install_plugin))
        .route("/plugins/{name}", delete(uninstall_plugin))
        .route("/plugins/{name}/start", post(start_plugin))
        .route("/plugins/{name}/stop", post(stop_plugin))
        .route("/plugins/{name}/command", post(send_command))
}
