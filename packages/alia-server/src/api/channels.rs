use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use sea_orm::EntityTrait;
use serde::Serialize;

use crate::auth::AuthUser;
use crate::channels::adapter::ChannelAdapter;
use crate::channels::registry::DisabledChannelInfo;
use crate::error::{AppError, Result};
use crate::module::factory;
use crate::module::lifecycle::ModuleContext;
use crate::state::AppState;

#[derive(Serialize)]
struct ChannelInfo {
    id: String,
    owner_user_id: Option<uuid::Uuid>,
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
            owner_user_id: channel.owner_user_id(),
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
    auth: AuthUser,
    Path(channel_id): Path<String>,
) -> Result<Json<RestartChannelResponse>> {
    if !auth.is_admin() {
        return Err(AppError::Forbidden);
    }
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

    if let Some(owner) = config.owner_user_id() {
        let user = crate::entity::user::Entity::find_by_id(owner)
            .one(&state.db)
            .await?;
        if !user.is_some_and(|user| user.status == "active") {
            return Err(AppError::Validation(
                "identity_unlinked: channel owner must reference an active account".into(),
            ));
        }
    }

    if state.channels.get(&channel_id).await.is_some() {
        state
            .channels
            .disable(&channel_id, "manual restart requested")
            .await
            .map_err(AppError::Internal)?;
    }

    let channel_type = config.channel_type();
    let mut channel = match factory::create_channel(&config, state.event_bus.clone()) {
        Ok(channel) => channel,
        Err(error) => {
            state
                .channels
                .mark_disabled(&channel_id, channel_type, error.to_string())
                .await;
            return Err(AppError::AiService(format!(
                "channel '{channel_id}' creation failed: {error}"
            )));
        }
    };
    let ctx = ModuleContext {
        db: state.db.clone(),
        config: state.config.clone(),
        event_bus: state.event_bus.clone(),
        cancel_token: state.cancel_token.clone(),
        settings: state.settings.clone(),
        artifacts: state.artifacts.clone(),
        plugin_storage: state.plugin_storage.clone(),
        sensors: state.sensors.clone(),
        scheduler: Some(state.modules.scheduler.clone()),
        conditions: Some(state.conditions.clone()),
        actions: Some(state.actions.clone()),
        conversation: Some(state.modules.conversation.clone()),
        memory: Some(state.modules.memory.clone()),
        llm: Some(state.modules.llm.clone()),
        minor_llm: Some(state.modules.minor_llm.clone()),
        major_llm: Some(state.modules.major_llm.clone()),
        transcript_correction_llm: state.modules.transcript_correction_llm.clone(),
        memory_capture_llm: state.modules.memory_capture_llm.clone(),
        sentence_expression_llm: state.modules.sentence_expression_llm.clone(),
        embedding: Some(state.modules.embedding.clone()),
        cache: Some(state.modules.cache.clone()),
        tools: Some(state.modules.tools.clone()),
        expression: Some(state.modules.expression.clone()),
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
