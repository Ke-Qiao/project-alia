use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde::Serialize;

use crate::auth::AuthUser;
use crate::channels::adapter::{ChannelAction, ChannelAdapter};
use crate::error::AppError;
use crate::error::Result;
use crate::state::AppState;

#[derive(Serialize)]
pub struct DeviceInfo {
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub status: String,
    pub endpoint: Option<String>,
    pub provisioning_mode: Option<bool>,
    pub bind_required: Option<bool>,
}

#[derive(Serialize)]
pub struct DeviceStatus {
    pub device_id: String,
    pub online: bool,
    pub cpu_percent: Option<f32>,
    pub memory_percent: Option<f32>,
    pub temperature_c: Option<f32>,
}

#[derive(Deserialize, Default)]
struct DeviceQuery {
    #[serde(default)]
    channel_id: Option<String>,
}

#[derive(Deserialize, Default)]
struct BindDeviceRequest {
    #[serde(default)]
    channel_id: Option<String>,
    #[serde(default)]
    server_id: Option<String>,
    #[serde(default)]
    server_name: Option<String>,
    #[serde(default)]
    server_endpoint: Option<String>,
    #[serde(default = "default_true")]
    require_confirmation: bool,
}

#[derive(Deserialize, Default)]
struct UnbindDeviceRequest {
    #[serde(default)]
    channel_id: Option<String>,
    #[serde(default)]
    server_id: Option<String>,
}

fn default_true() -> bool {
    true
}

async fn get_device(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<DeviceQuery>,
) -> Result<Json<Option<DeviceInfo>>> {
    let channel = resolve_embedded_channel(&state, query.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "discovery_status".to_string(),
            params: serde_json::json!({}),
        })
        .await?;

    let candidate = response
        .get("candidate")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if candidate.is_null() {
        return Ok(Json(None));
    }

    Ok(Json(Some(DeviceInfo {
        device_id: candidate
            .get("device_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        device_name: candidate
            .get("device_name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        status: candidate
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        endpoint: candidate
            .get("endpoint")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        provisioning_mode: candidate
            .get("provisioning_mode")
            .and_then(serde_json::Value::as_bool),
        bind_required: candidate
            .get("bind_required")
            .and_then(serde_json::Value::as_bool),
    })))
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
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<BindDeviceRequest>,
) -> Result<Json<DeviceInfo>> {
    let channel = resolve_embedded_channel(&state, body.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "bind_device".to_string(),
            params: serde_json::json!({
                "server_id": body.server_id,
                "server_name": body.server_name,
                "server_endpoint": body.server_endpoint,
                "require_confirmation": body.require_confirmation,
            }),
        })
        .await?;

    Ok(Json(DeviceInfo {
        device_id: response
            .get("device_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        device_name: response
            .get("device_name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        status: if response
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            "bound".to_string()
        } else {
            "bind_failed".to_string()
        },
        endpoint: None,
        provisioning_mode: None,
        bind_required: Some(false),
    }))
}

async fn unbind_device(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(body): Json<UnbindDeviceRequest>,
) -> Result<Json<serde_json::Value>> {
    let channel = resolve_embedded_channel(&state, body.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "unbind_device".to_string(),
            params: serde_json::json!({
                "server_id": body.server_id.unwrap_or_else(|| state.config.server.id.clone()),
            }),
        })
        .await?;
    Ok(Json(response))
}

async fn get_device_settings(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<DeviceQuery>,
) -> Result<Json<serde_json::Value>> {
    let channel = resolve_embedded_channel(&state, query.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "get_settings".to_string(),
            params: serde_json::json!({}),
        })
        .await?;
    Ok(Json(response))
}

async fn update_device_settings(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<DeviceQuery>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>> {
    let channel = resolve_embedded_channel(&state, query.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "update_settings".to_string(),
            params: body,
        })
        .await?;
    Ok(Json(response))
}

async fn list_direct_responses(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<DeviceQuery>,
) -> Result<Json<serde_json::Value>> {
    let channel = resolve_embedded_channel(&state, query.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "list_direct_responses".to_string(),
            params: serde_json::json!({}),
        })
        .await?;
    Ok(Json(response))
}

async fn upsert_direct_response(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<DeviceQuery>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>> {
    let channel = resolve_embedded_channel(&state, query.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "upsert_direct_response".to_string(),
            params: body,
        })
        .await?;
    Ok(Json(response))
}

async fn delete_direct_response(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<DeviceQuery>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>> {
    let channel = resolve_embedded_channel(&state, query.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "delete_direct_response".to_string(),
            params: serde_json::json!({ "id": id }),
        })
        .await?;
    Ok(Json(response))
}

async fn resolve_embedded_channel(
    state: &AppState,
    channel_id: Option<&str>,
) -> Result<Arc<dyn ChannelAdapter>> {
    if let Some(channel_id) = channel_id {
        let channel = state
            .channels
            .get(channel_id)
            .await
            .ok_or_else(|| AppError::NotFound(format!("channel '{channel_id}' not found")))?;
        if channel.channel_type() != "embedded" {
            return Err(AppError::Validation(format!(
                "channel '{channel_id}' is not an embedded channel"
            )));
        }
        return Ok(channel);
    }

    state
        .channels
        .list()
        .await
        .into_iter()
        .find(|channel| channel.channel_type() == "embedded")
        .ok_or_else(|| AppError::NotFound("no embedded channel registered".to_string()))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/device", get(get_device))
        .route("/device/status", get(get_device_status))
        .route("/device/bind", post(bind_device))
        .route("/device/unbind", post(unbind_device))
        .route(
            "/device/settings",
            get(get_device_settings).patch(update_device_settings),
        )
        .route(
            "/device/settings/direct-responses",
            get(list_direct_responses).post(upsert_direct_response),
        )
        .route(
            "/device/settings/direct-responses/{id}",
            delete(delete_direct_response),
        )
}
