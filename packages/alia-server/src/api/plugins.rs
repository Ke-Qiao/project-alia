use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::Deserialize;
use serde::Serialize;

use crate::auth::AuthUser;
use crate::channels::adapter::{ChannelAction, ChannelAdapter};
use crate::error::{AppError, Result};
use crate::state::AppState;

#[derive(Serialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub plugin_type: String,
    pub state: String,
}

#[derive(Deserialize, Default)]
struct PluginQuery {
    #[serde(default)]
    channel_id: Option<String>,
}

#[derive(Deserialize, Default)]
struct CommandBody {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

async fn list_plugins(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<PluginQuery>,
) -> Result<Json<Vec<PluginInfo>>> {
    let channel = resolve_embedded_channel(&state, query.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "list_plugins".to_string(),
            params: serde_json::json!({}),
        })
        .await?;
    let plugins = response["plugins"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|plugin| PluginInfo {
            name: plugin["name"].as_str().unwrap_or_default().to_string(),
            version: plugin["version"].as_str().unwrap_or_default().to_string(),
            plugin_type: plugin["plugin_type"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            state: plugin["state"].to_string(),
        })
        .collect();
    Ok(Json(plugins))
}

async fn install_plugin(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<PluginQuery>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>> {
    let channel = resolve_embedded_channel(&state, query.channel_id.as_deref()).await?;
    let upload_name = format!("upload-{}", uuid::Uuid::new_v4());
    let response = channel
        .execute_action(ChannelAction {
            name: "install_plugin".to_string(),
            params: serde_json::json!({
                "name": upload_name,
                "version": "",
                "zip_base64": base64::engine::general_purpose::STANDARD.encode(body),
            }),
        })
        .await?;
    Ok(Json(response))
}

async fn uninstall_plugin(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<PluginQuery>,
    Path(_name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let channel = resolve_embedded_channel(&state, query.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "uninstall_plugin".to_string(),
            params: serde_json::json!({ "name": _name }),
        })
        .await?;
    Ok(Json(response))
}

async fn start_plugin(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<PluginQuery>,
    Path(_name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let channel = resolve_embedded_channel(&state, query.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "start_plugin".to_string(),
            params: serde_json::json!({ "name": _name }),
        })
        .await?;
    Ok(Json(response))
}

async fn stop_plugin(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<PluginQuery>,
    Path(_name): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let channel = resolve_embedded_channel(&state, query.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "stop_plugin".to_string(),
            params: serde_json::json!({ "name": _name }),
        })
        .await?;
    Ok(Json(response))
}

async fn send_command(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(query): Query<PluginQuery>,
    Path(_name): Path<String>,
    Json(body): Json<CommandBody>,
) -> Result<Json<serde_json::Value>> {
    let channel = resolve_embedded_channel(&state, query.channel_id.as_deref()).await?;
    let response = channel
        .execute_action(ChannelAction {
            name: "send_plugin_command".to_string(),
            params: serde_json::json!({
                "plugin": _name,
                "action": body.action.unwrap_or_else(|| "command".to_string()),
                "payload": body.payload.unwrap_or_else(|| serde_json::json!({})).to_string(),
            }),
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
        .route("/plugins", get(list_plugins))
        .route("/plugins/install", post(install_plugin))
        .route("/plugins/{name}", delete(uninstall_plugin))
        .route("/plugins/{name}/start", post(start_plugin))
        .route("/plugins/{name}/stop", post(stop_plugin))
        .route("/plugins/{name}/command", post(send_command))
}
