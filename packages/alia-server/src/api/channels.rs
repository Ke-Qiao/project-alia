use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::AuthUser;
use crate::channels::adapter::ChannelAdapter;
use crate::channels::ensure_default_wakeword_response;
use crate::channels::registry::DisabledChannelInfo;
use crate::error::{AppError, Result};
use crate::module::factory;
use crate::module::lifecycle::ModuleContext;
use crate::state::AppState;

#[derive(Serialize)]
struct ChannelInfo {
    id: String,
    channel_type: String,
    active: bool,
    health: String,
}

#[derive(Serialize)]
struct ChannelsResponse {
    active: Vec<ChannelInfo>,
    disabled: Vec<DisabledChannelInfo>,
}

#[derive(Serialize)]
struct RestartChannelResponse {
    id: String,
    channel_type: String,
    active: bool,
}

async fn list_channels(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<ChannelsResponse>> {
    let mut active = Vec::new();
    for channel in state.channels.list().await {
        active.push(ChannelInfo {
            id: channel.channel_id().to_string(),
            channel_type: channel.channel_type().to_string(),
            active: true,
            health: format!("{:?}", channel.health_check().await),
        });
    }

    Ok(Json(ChannelsResponse {
        active,
        disabled: state.channels.disabled().await,
    }))
}

async fn restart_channel(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(channel_id): Path<String>,
) -> Result<Json<RestartChannelResponse>> {
    let config = state
        .config
        .channels
        .iter()
        .find(|channel| channel.id() == channel_id)
        .ok_or_else(|| AppError::NotFound(format!("channel '{channel_id}' not found in config")))?
        .clone();

    if !config.enabled() {
        return Err(AppError::Validation(format!(
            "channel '{channel_id}' is disabled in config"
        )));
    }

    if state.channels.get(&channel_id).await.is_some() {
        state
            .channels
            .disable(&channel_id, "manual restart requested")
            .await
            .map_err(AppError::Internal)?;
    }

    let channel_type = config.channel_type();
    let mut channel = factory::create_channel(&config, state.event_bus.clone());
    let ctx = ModuleContext {
        db: state.db.clone(),
        config: state.config.clone(),
        event_bus: state.event_bus.clone(),
        cancel_token: state.cancel_token.clone(),
        scheduler: Some(state.modules.scheduler.clone()),
        conditions: Some(state.conditions.clone()),
        conversation: Some(state.modules.conversation.clone()),
    };

    if let Err(error) = channel.init(&ctx).await {
        state
            .channels
            .mark_disabled(&channel_id, channel_type, error.to_string())
            .await;
        return Err(AppError::AiService(format!(
            "channel '{channel_id}' init failed: {error}"
        )));
    }

    let channel: Arc<dyn ChannelAdapter> = Arc::from(channel);
    if let Err(error) = state.channels.register(channel.clone()).await {
        state
            .channels
            .mark_disabled(&channel_id, channel_type, error.to_string())
            .await;
        return Err(AppError::AiService(format!(
            "channel '{channel_id}' registration failed: {error}"
        )));
    }

    if let Err(error) = channel.start().await {
        let _ = state.channels.disable(&channel_id, error.to_string()).await;
        return Err(AppError::AiService(format!(
            "channel '{channel_id}' start failed: {error}"
        )));
    }
    if channel.channel_type() == "embedded" {
        ensure_default_wakeword_response(channel.as_ref(), state.modules.tts.as_ref()).await;
    }

    Ok(Json(RestartChannelResponse {
        id: channel_id,
        channel_type: channel_type.to_string(),
        active: true,
    }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/channels", get(list_channels))
        .route("/channels/{channel_id}/restart", post(restart_channel))
}
