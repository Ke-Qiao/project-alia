//! Server assembly and lifecycle, independent of process entry points.
//!
//! Hosts configure logging and request shutdown through a CancellationToken.
//! This module does not install process signal handlers or terminate the process.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use axum::Router;
use axum::routing::get;
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

use crate::auth::ensure_server_keypair;
use crate::config::ServerConfig;
use crate::discovery::{ServerDiscoveryAdvertisement, ServerDiscoveryAnnouncement};
use crate::event::EventBus;
use crate::module::ServerBuilder;
use crate::modules::platform::migrations;
use crate::{api, openapi, ws};

mod setup;
pub use setup::run_setup;

const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Run the server until the caller cancels `shutdown` or a listener fails.
///
/// HTTP and gRPC sockets are bound before modules start. Listener failures
/// cancel sibling services and are returned after channels, modules, discovery,
/// and the database pool are shut down. The caller's token is not cancelled by
/// internal failures, allowing another instance to be started by the same host.
pub async fn run(config: Arc<ServerConfig>, shutdown: CancellationToken) -> anyhow::Result<()> {
    let cancel = shutdown.child_token();
    let _cancel_on_drop = cancel.clone().drop_guard();
    if cancel.is_cancelled() {
        return Ok(());
    }

    ensure_server_keypair(&config.auth.server_keypair_path).with_context(|| {
        format!(
            "failed to ensure server keypair at {}",
            config.auth.server_keypair_path.display()
        )
    })?;

    let host = config
        .server
        .host
        .trim_start_matches('[')
        .trim_end_matches(']');
    let http = TcpListener::bind((host, config.server.port))
        .await
        .context("binding server HTTP listener")?;
    #[cfg(feature = "channel-embedded")]
    let grpc = {
        let port = config
            .server
            .port
            .checked_add(1)
            .context("HTTP port must be below 65535 to reserve the adjacent gRPC port")?;
        TcpListener::bind((host, port))
            .await
            .context("binding embedded audio ingress gRPC listener")?
    };

    let mut options = ConnectOptions::new(&config.database.url);
    options
        .max_connections(config.database.max_connections)
        .min_connections(config.database.min_connections);
    let db = tokio::select! {
        _ = cancel.cancelled() => return Ok(()),
        result = Database::connect(options) => result.context("connecting to PostgreSQL")?,
    };

    let result = run_connected(
        config,
        db.clone(),
        http,
        #[cfg(feature = "channel-embedded")]
        grpc,
        cancel.clone(),
    )
    .await;
    cancel.cancel();
    let close_result = tokio::time::timeout(SHUTDOWN_GRACE, db.close())
        .await
        .context("timed out closing PostgreSQL pool")
        .and_then(|result| result.context("closing PostgreSQL pool"));
    if let Err(error) = &close_result {
        tracing::error!(error = %error, "database cleanup failed");
    }
    result.and(close_result)
}

async fn run_connected(
    config: Arc<ServerConfig>,
    db: DatabaseConnection,
    http: TcpListener,
    #[cfg(feature = "channel-embedded")] grpc: TcpListener,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    tokio::select! {
        _ = cancel.cancelled() => return Ok(()),
        result = migrations::run(&db) => { result.context("preparing server database")?; }
    }

    let event_bus = EventBus::new(1024);
    let state = ServerBuilder::new(config.clone(), db, event_bus.clone(), cancel.clone())
        .build()
        .await
        .context("initializing server modules")?;
    let app = Router::new()
        .merge(api::routes())
        .route("/api/v1/ws", get(ws::ws_handler))
        .merge(Scalar::with_url("/scalar", openapi::ApiDoc::openapi()))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());
    let app = attach_frontend(app);

    let discovery = match ServerDiscoveryAdvertisement::start(
        ServerDiscoveryAnnouncement::from_config(&config),
    ) {
        Ok(discovery) => discovery,
        Err(error) => {
            tracing::warn!(error = %error, "server discovery unavailable");
            None
        }
    };

    let mut services = JoinSet::new();
    let http_cancel = cancel.clone();
    tracing::info!(addr = ?http.local_addr(), "server HTTP listening");
    services.spawn(async move {
        axum::serve(http, app)
            .with_graceful_shutdown(http_cancel.cancelled_owned())
            .await
            .context("serving server HTTP")
    });

    #[cfg(feature = "channel-embedded")]
    {
        use crate::grpc::audio_ingress::ServerAudioIngressService;
        use crate::grpc::voice_session::VoiceSessionService;
        use alia_proto::audio::server_audio_ingress_server::ServerAudioIngressServer;
        use alia_proto::voice::voice_session_server::VoiceSessionServer;

        let grpc_cancel = cancel.clone();
        let channels = state.channels.clone();
        let auth = ServerAudioIngressService::new(event_bus, channels.clone(), "embedded");
        let voice = VoiceSessionService::new(
            auth.clone(),
            channels,
            state.turn_detector.clone(),
            state.voice_input.clone(),
            grpc_cancel.clone(),
        );
        tracing::info!(addr = ?grpc.local_addr(), "embedded audio ingress gRPC listening");
        services.spawn(async move {
            let incoming = async_stream::stream! {
                loop {
                    yield grpc.accept().await.map(|(stream, _)| stream);
                }
            };
            tonic::transport::Server::builder()
                .add_service(
                    ServerAudioIngressServer::new(auth).max_decoding_message_size(64 * 1024),
                )
                .add_service(VoiceSessionServer::new(voice).max_decoding_message_size(64 * 1024))
                .serve_with_incoming_shutdown(incoming, grpc_cancel.cancelled_owned())
                .await
                .context("serving embedded audio ingress gRPC")
        });
    }

    let result = supervise(&mut services, &cancel).await;
    cancel.cancel();
    if let Some(discovery) = discovery {
        discovery.shutdown();
    }
    let drain_result = drain(&mut services).await;
    // Stop interaction waits before draining the independently owned effects.
    let pipeline_result = if let Some(task) = state.pipeline_task.lock().await.take() {
        task.await.context("joining interaction pipeline")
    } else {
        Ok(())
    };
    // Outbound channels and plugins remain usable until dispatched effects
    // have persisted their receipts or reported a durable-recovery requirement.
    let execution_result = state.execution.shutdown().await;
    let channels_result = tokio::time::timeout(SHUTDOWN_GRACE, state.channels.shutdown())
        .await
        .context("timed out stopping server channels");
    // Still attempt module cleanup if a channel does not honor shutdown.
    let modules_result = state.modules.shutdown().await;
    for error in [
        &drain_result,
        &pipeline_result,
        &execution_result,
        &channels_result,
        &modules_result,
    ]
    .into_iter()
    .filter_map(|result| result.as_ref().err())
    {
        tracing::error!(error = %error, "server cleanup failed");
    }
    let result = result
        .and(drain_result)
        .and(pipeline_result)
        .and(execution_result)
        .and(channels_result)
        .and(modules_result);
    if result.is_ok() {
        tracing::info!("server shutdown complete");
    }
    result
}

async fn supervise(
    services: &mut JoinSet<anyhow::Result<()>>,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    tokio::select! {
        _ = cancel.cancelled() => Ok(()),
        finished = services.join_next() => match finished {
            Some(Ok(Err(error))) => Err(error),
            Some(Err(error)) => Err(error).context("server listener task failed"),
            Some(Ok(Ok(()))) if cancel.is_cancelled() => Ok(()),
            _ => Err(anyhow!("server listener stopped before shutdown was requested")),
        },
    }
}

async fn drain(services: &mut JoinSet<anyhow::Result<()>>) -> anyhow::Result<()> {
    let result = tokio::time::timeout(SHUTDOWN_GRACE, async {
        let mut first_error = None;
        while let Some(result) = services.join_next().await {
            let result = result
                .context("joining server listener task")
                .and_then(|value| value);
            if first_error.is_none() {
                first_error = result.err();
            }
        }
        first_error.map_or(Ok(()), Err)
    })
    .await;
    match result {
        Ok(result) => result,
        Err(error) => {
            services.shutdown().await;
            Err(error).context("timed out draining server listeners")
        }
    }
}

fn attach_frontend<S>(app: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let dist = std::env::var_os("ALIA_FRONTEND_DIST")
        .map(PathBuf::from)
        .into_iter()
        .chain([
            PathBuf::from("packages/alia-frontend/dist"),
            PathBuf::from("alia-frontend/dist"),
            PathBuf::from("dist"),
        ])
        .find(|path| path.join("index.html").is_file());
    match dist {
        Some(dist) => {
            let index = dist.join("index.html");
            app.fallback_service(ServeDir::new(dist).not_found_service(ServeFile::new(index)))
        }
        None => app,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn listener_failure_is_returned_and_siblings_can_be_drained() {
        let cancel = CancellationToken::new();
        let mut services = JoinSet::new();
        let sibling_cancel = cancel.clone();
        services.spawn(async move {
            sibling_cancel.cancelled().await;
            Ok(())
        });
        services.spawn(async { anyhow::bail!("gRPC listener failed") });
        let error = supervise(&mut services, &cancel).await.unwrap_err();
        assert_eq!(error.to_string(), "gRPC listener failed");
        cancel.cancel();
        drain(&mut services).await.unwrap();
        assert!(services.is_empty());
    }

    #[tokio::test]
    async fn cancellation_drains_services_without_cancelling_parent() {
        let parent = CancellationToken::new();
        let cancel = parent.child_token();
        let mut services = JoinSet::new();
        let service_cancel = cancel.clone();
        services.spawn(async move {
            service_cancel.cancelled().await;
            Ok(())
        });
        cancel.cancel();
        supervise(&mut services, &cancel).await.unwrap();
        drain(&mut services).await.unwrap();
        assert!(!parent.is_cancelled());
    }
}
