use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::FutureExt;
use sea_orm::EntityTrait;
use serde::Serialize;

use crate::auth::AuthUser;
use crate::bootstrap::factory;
use crate::channels::adapter::ChannelAdapter;
use crate::channels::registry::DisabledChannelInfo;
use crate::error::{AppError, Result};
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
    let (reply, result) = tokio::sync::oneshot::channel();
    let tasks = state.runtime_tasks.clone();
    tasks
        .try_spawn("restart_channel", async move {
            let mut cleanup_errors = Vec::new();
            let outcome = restart_owned(state, channel_id, &mut cleanup_errors).await;
            finish_restart(reply, outcome, cleanup_errors)
        })
        .map_err(|_| AppError::AiService("server is stopping".into()))?;
    result
        .await
        .map_err(|_| AppError::AiService("channel restart task failed".into()))?
}

fn finish_restart(
    reply: tokio::sync::oneshot::Sender<Result<Json<RestartChannelResponse>>>,
    outcome: Result<Json<RestartChannelResponse>>,
    cleanup_errors: Vec<String>,
) -> anyhow::Result<()> {
    let _ = reply.send(outcome);
    anyhow::ensure!(
        cleanup_errors.is_empty(),
        "channel restart cleanup failed: {}",
        cleanup_errors.join("; ")
    );
    Ok(())
}

/// An unregistered adapter has no registry owner yet. Retain it and its stop
/// future if cleanup panics: the runtime must not release its dependencies.
async fn rollback_channel(
    channel: &dyn ChannelAdapter,
    cleanup_errors: &mut Vec<String>,
) -> anyhow::Result<()> {
    let mut stop = Box::pin(channel.stop());
    match AssertUnwindSafe(stop.as_mut()).catch_unwind().await {
        Ok(result) => {
            if let Err(error) = &result {
                cleanup_errors.push(format!("{}: {error:#}", channel.channel_id()));
            }
            result
        }
        Err(_) => {
            tracing::error!(
                channel = channel.channel_id(),
                "channel rollback panicked; retaining cleanup and dependencies"
            );
            std::future::pending().await
        }
    }
}

/// The runtime owns initialization and rollback independently of the HTTP caller.
async fn restart_owned(
    state: AppState,
    channel_id: String,
    cleanup_errors: &mut Vec<String>,
) -> Result<Json<RestartChannelResponse>> {
    if state.cancel_token.is_cancelled() || state.channels.is_closing() {
        return Err(AppError::AiService("server is stopping".into()));
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

    let _restart = state
        .channels
        .restart_guard(&channel_id)
        .await
        .map_err(AppError::Internal)?;

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
    let mut channel = match factory::create_channel(
        &config,
        state.event_bus.clone(),
        state.db.clone(),
        state.cancel_token.clone(),
        state.modules.conversation.clone(),
        state.sensors.clone(),
        &state.config,
    ) {
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

    let initialized = tokio::select! {
        biased;
        _ = state.cancel_token.cancelled() => Err(anyhow::anyhow!("channel initialization cancelled")),
        result = AssertUnwindSafe(channel.init()).catch_unwind() => match result {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("channel initialization panicked")),
        },
    };
    if let Err(error) = initialized {
        let error = match rollback_channel(channel.as_ref(), cleanup_errors).await {
            Ok(()) => error,
            Err(cleanup) => error.context(format!(
                "channel initialization rollback failed: {cleanup:#}"
            )),
        };
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
        let error = match rollback_channel(channel.as_ref(), cleanup_errors).await {
            Ok(()) => error,
            Err(cleanup) => {
                error.context(format!("channel registration rollback failed: {cleanup:#}"))
            }
        };
        state
            .channels
            .mark_disabled(&channel_id, channel_type, error.to_string())
            .await;
        return Err(AppError::AiService(format!(
            "channel '{channel_id}' registration failed: {error}"
        )));
    }

    if let Err(error) = state.channels.start(&channel_id).await {
        let error = match state.channels.disable(&channel_id, error.to_string()).await {
            Ok(()) => error,
            Err(cleanup) => {
                cleanup_errors.push(format!("{channel_id}: {cleanup:#}"));
                error.context(format!("channel startup rollback failed: {cleanup:#}"))
            }
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::adapter::ChannelCapabilities;
    use crate::modules::platform::lifecycle::ModuleLifecycle;
    use crate::modules::platform::lifecycle::RuntimeTasks;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;

    struct FailingCleanup {
        entered: Arc<Notify>,
        release: Arc<Notify>,
        joined: Arc<AtomicBool>,
    }

    #[async_trait]
    impl ModuleLifecycle for FailingCleanup {
        fn name(&self) -> &'static str {
            "failing_cleanup"
        }

        async fn stop(&self) -> anyhow::Result<()> {
            self.entered.notify_one();
            self.release.notified().await;
            self.joined.store(true, Ordering::SeqCst);
            anyhow::bail!("injected joined rollback failure")
        }
    }

    #[async_trait]
    impl ChannelAdapter for FailingCleanup {
        fn channel_type(&self) -> &'static str {
            "test"
        }
        fn channel_id(&self) -> &str {
            "restart-test"
        }
        fn capabilities(&self) -> ChannelCapabilities {
            ChannelCapabilities::default()
        }
    }

    #[tokio::test]
    async fn dropped_http_receiver_does_not_hide_restart_rollback_failure() {
        let tasks = RuntimeTasks::default();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let joined = Arc::new(AtomicBool::new(false));
        let channel = FailingCleanup {
            entered: entered.clone(),
            release: release.clone(),
            joined: joined.clone(),
        };
        let (reply, receiver) = tokio::sync::oneshot::channel();
        tasks
            .try_spawn("restart_channel", async move {
                let mut errors = Vec::new();
                rollback_channel(&channel, &mut errors).await.unwrap_err();
                finish_restart(
                    reply,
                    Err(AppError::AiService("init failed".into())),
                    errors,
                )
            })
            .unwrap();
        entered.notified().await;
        drop(receiver);
        tasks.close();
        assert!(!joined.load(Ordering::SeqCst));
        release.notify_one();
        let error = tokio::time::timeout(std::time::Duration::from_secs(2), tasks.wait())
            .await
            .unwrap()
            .unwrap_err();
        assert!(joined.load(Ordering::SeqCst));
        assert_eq!(tasks.active(), 0);
        assert!(format!("{error:#}").contains("injected joined rollback failure"));
    }

    #[tokio::test]
    async fn rejected_restart_request_does_not_poison_runtime_shutdown() {
        let tasks = RuntimeTasks::default();
        let (reply, receiver) = tokio::sync::oneshot::channel();
        tasks
            .try_spawn("restart_channel", async move {
                finish_restart(
                    reply,
                    Err(AppError::NotFound("missing channel".into())),
                    vec![],
                )
            })
            .unwrap();
        assert!(matches!(
            receiver.await.unwrap(),
            Err(AppError::NotFound(_))
        ));
        tasks.close();
        tasks.wait().await.unwrap();
    }
}
