use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::AuthUser;
use crate::error::Result;
use crate::state::AppState;

#[derive(Serialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub status: String,
}

#[derive(Serialize)]
pub struct DeviceStatus {
    pub device_id: String,
    pub online: bool,
    pub cpu_percent: Option<f32>,
    pub memory_percent: Option<f32>,
    pub temperature_c: Option<f32>,
}

async fn get_device(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Option<DeviceInfo>>> {
    // TODO: query bound device
    Ok(Json(None))
}

async fn get_device_status(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<DeviceStatus>> {
    Ok(Json(DeviceStatus {
        device_id: String::new(),
        online: false,
        cpu_percent: None,
        memory_percent: None,
        temperature_c: None,
    }))
}

async fn bind_device(
    State(_state): State<AppState>,
    _auth: AuthUser,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<DeviceInfo>> {
    // TODO: forward gRPC Bind
    Ok(Json(DeviceInfo {
        device_id: String::new(),
        device_name: String::new(),
        status: "binding".into(),
    }))
}

async fn unbind_device(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<serde_json::Value>> {
    // TODO: forward gRPC Unbind
    Ok(Json(serde_json::json!({"success": true})))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/device", get(get_device))
        .route("/device/status", get(get_device_status))
        .route("/device/bind", post(bind_device))
        .route("/device/unbind", post(unbind_device))
}
