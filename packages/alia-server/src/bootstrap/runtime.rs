use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use alia_transport::connections::{Connection, Connections};
use anyhow::{Context, Result};
use axum::{Router, routing::get};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tower_http::{
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

use crate::auth::ensure_server_keypair;
use crate::config::ServerConfig;
use crate::discovery::{ServerDiscoveryAdvertisement, ServerDiscoveryAnnouncement};
use crate::module::lifecycle::{
    RuntimeCleanup, RuntimeTasks, ShutdownHandle, ShutdownPhase, startup_step,
};
use crate::state::AppState;
use crate::{api, openapi, ws};

#[derive(Default)]
struct Services {
    task: Mutex<Option<JoinHandle<Result<()>>>>,
    result: Mutex<Option<std::result::Result<(), String>>>,
}
impl Services {
    async fn join(&self) -> Result<()> {
        let mut task = self.task.lock().await;
        if let Some(handle) = task.as_mut() {
            let result = handle
                .await
                .context("server listener owner panicked")
                .and_then(|result| result);
            *self.result.lock().await = Some(result.map_err(|error| format!("{error:#}")));
            task.take();
        }
        match self.result.lock().await.as_ref() {
            Some(Err(error)) => Err(anyhow::anyhow!(error.clone())),
            _ => Ok(()),
        }
    }
}

pub(crate) struct RuntimeWiring {
    http: Arc<std::sync::Mutex<Option<TcpListener>>>,
    #[cfg(feature = "channel-embedded")]
    grpc: Arc<std::sync::Mutex<Option<TcpListener>>>,
    http_addr: SocketAddr,
    grpc_addr: Option<SocketAddr>,
    connections: Arc<Connections>,
    services: Arc<Services>,
    pub(crate) tasks: Arc<RuntimeTasks>,
}

impl RuntimeWiring {
    pub(crate) async fn bind(
        config: Arc<ServerConfig>,
        cancel: CancellationToken,
        cleanup: Arc<RuntimeCleanup>,
    ) -> Result<(Self, Arc<ServerConfig>)> {
        let host = config
            .server
            .host
            .trim_start_matches('[')
            .trim_end_matches(']');
        let http = startup_step(&cancel, "binding HTTP listener", async {
            TcpListener::bind((host, config.server.port))
                .await
                .map_err(Into::into)
        })
        .await?;
        let http_addr = http.local_addr()?;
        #[cfg(feature = "channel-embedded")]
        let grpc = {
            let port = match config.server.grpc_port {
                Some(port) => port,
                None if config.server.port == 0 => 0,
                None => config
                    .server
                    .port
                    .checked_add(1)
                    .context("HTTP port 65535 requires explicit grpc_port")?,
            };
            startup_step(&cancel, "binding gRPC listener", async {
                TcpListener::bind((host, port)).await.map_err(Into::into)
            })
            .await?
        };
        #[cfg(feature = "channel-embedded")]
        let grpc_addr = Some(grpc.local_addr()?);
        #[cfg(not(feature = "channel-embedded"))]
        let grpc_addr: Option<SocketAddr> = None;
        let http = Arc::new(std::sync::Mutex::new(Some(http)));
        #[cfg(feature = "channel-embedded")]
        let grpc = Arc::new(std::sync::Mutex::new(Some(grpc)));
        let close_http = http.clone();
        #[cfg(feature = "channel-embedded")]
        let close_grpc = grpc.clone();
        let connections = Connections::new();
        let services = Arc::new(Services::default());
        let tasks = Arc::new(RuntimeTasks::default());
        let owner = (connections.clone(), services.clone(), tasks.clone());
        let drain = Duration::from_millis(config.server.drain_timeout_ms);
        cleanup.add("runtime_ingress", ShutdownPhase::Ingress, move || {
            let (connections, services, tasks) = owner.clone();
            let close_http = close_http.clone();
            #[cfg(feature = "channel-embedded")]
            let close_grpc = close_grpc.clone();
            async move {
                close_http.lock().expect("HTTP listener mutex").take();
                #[cfg(feature = "channel-embedded")]
                close_grpc.lock().expect("gRPC listener mutex").take();
                tasks.close();
                // The join handle stays in Services across timeout. Closing accepted
                // sockets also reaches Tonic's internal HTTP/2 and upgraded WS tasks.
                let _ = tokio::time::timeout(drain, services.join()).await;
                connections.close();
                let serving = services.join().await;
                let sockets = connections
                    .wait_closed()
                    .await
                    .context("joining server connections");
                let actors = tasks.wait().await;
                let failures = [serving, sockets, actors]
                    .into_iter()
                    .filter_map(|result| result.err().map(|error| format!("{error:#}")))
                    .collect::<Vec<_>>();
                anyhow::ensure!(
                    failures.is_empty(),
                    "ingress cleanup failed: {}",
                    failures.join("; ")
                );
                Ok(())
            }
        });
        ensure_server_keypair(&config.auth.server_keypair_path)
            .context("preparing server identity")?;
        let mut effective = (*config).clone();
        effective.server.port = http_addr.port();
        effective.server.grpc_port = grpc_addr.map(|addr| addr.port());
        Ok((
            Self {
                http,
                #[cfg(feature = "channel-embedded")]
                grpc,
                http_addr,
                grpc_addr,
                connections,
                services,
                tasks,
            },
            Arc::new(effective),
        ))
    }
}

/// Owns one assembled instance. Clone `state()` for handlers; keep this owner
/// for its lifecycle. Dropping it requests cleanup without dropping task joins.
pub struct ServerRuntime {
    state: AppState,
    pub requested_config: Arc<ServerConfig>,
    cleanup: Arc<RuntimeCleanup>,
    wiring: RuntimeWiring,
}

impl ServerRuntime {
    pub(crate) async fn assembled(
        state: AppState,
        requested_config: Arc<ServerConfig>,
        cleanup: Arc<RuntimeCleanup>,
        wiring: RuntimeWiring,
    ) -> Result<Self> {
        if let Some(discovery) = ServerDiscoveryAdvertisement::start(
            ServerDiscoveryAnnouncement::from_config(&state.config),
        )
        .await?
        {
            let discovery = Arc::new(discovery);
            cleanup.add("discovery", ShutdownPhase::Ingress, move || {
                let discovery = discovery.clone();
                async move { discovery.shutdown().await }
            });
        }
        Ok(Self {
            state,
            requested_config,
            cleanup,
            wiring,
        })
    }
    pub fn state(&self) -> &AppState {
        &self.state
    }
    pub fn http_addr(&self) -> SocketAddr {
        self.wiring.http_addr
    }
    pub fn grpc_addr(&self) -> Option<SocketAddr> {
        self.wiring.grpc_addr
    }
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.cleanup.handle()
    }
    pub fn active_connections(&self) -> usize {
        self.wiring.connections.active_count()
    }

    pub async fn shutdown(self) -> Result<()> {
        self.request_shutdown();
        self.cleanup
            .start()
            .wait_until(self.cleanup.deadline())
            .await
    }

    fn request_shutdown(&self) {
        self.state.channels.close_admission();
        self.state.runtime_tasks.close();
        self.state.cancel_token.cancel();
    }

    pub async fn run(self) -> Result<()> {
        let app = Router::new()
            .merge(api::routes())
            .route("/api/v1/ws", get(ws::ws_handler))
            .merge(Scalar::with_url("/scalar", openapi::ApiDoc::openapi()))
            .layer(CorsLayer::permissive())
            .with_state(self.state.clone());
        let app = attach_frontend(app);
        let http = self
            .wiring
            .http
            .lock()
            .expect("HTTP listener mutex")
            .take()
            .context("HTTP listener already consumed")?;
        let connections = self.wiring.connections.clone();
        let cancel = self.state.cancel_token.clone();
        let owner_cancel = cancel.clone();
        #[cfg(feature = "channel-embedded")]
        let grpc = self
            .wiring
            .grpc
            .lock()
            .expect("gRPC listener mutex")
            .take()
            .context("gRPC listener already consumed")?;
        #[cfg(feature = "channel-embedded")]
        let state = self.state.clone();
        let mut service_slot = self.wiring.services.task.lock().await;
        let task = tokio::spawn(async move {
            let mut listeners = JoinSet::new();
            let http_cancel = cancel.clone();
            let http_connections = connections.clone();
            listeners.spawn(async move {
                axum::serve(
                    HttpListener {
                        listener: http,
                        connections: http_connections,
                    },
                    app,
                )
                .with_graceful_shutdown(http_cancel.cancelled_owned())
                .await
                .context("serving server HTTP")
            });
            #[cfg(feature = "channel-embedded")]
            {
                use crate::grpc::{
                    audio_ingress::ServerAudioIngressService, voice_session::VoiceSessionService,
                };
                use alia_proto::audio::server_audio_ingress_server::ServerAudioIngressServer;
                use alia_proto::voice::voice_session_server::VoiceSessionServer;
                let auth = ServerAudioIngressService::new(
                    state.event_bus.clone(),
                    state.channels.clone(),
                    "embedded",
                );
                let voice = VoiceSessionService::new(
                    auth.clone(),
                    state.turn_detector.clone(),
                    state.voice_input.clone(),
                    state.delivery.clone(),
                    Duration::from_millis(state.config.policies.duplex.hold_budget_ms),
                    cancel.clone(),
                    state.runtime_tasks.clone(),
                )
                .with_initiatives(
                    state.voice_initiatives.clone(),
                    state.modules.context.clone(),
                );
                let grpc_cancel = cancel.clone();
                listeners.spawn(async move {
                    let incoming = async_stream::stream! {
                        loop {
                            match grpc.accept().await {
                                Ok((stream, _)) => yield connections.wrap(stream),
                                Err(error) => yield Err(error),
                            }
                        }
                    };
                    tonic::transport::Server::builder()
                        .add_service(
                            ServerAudioIngressServer::new(auth)
                                .max_decoding_message_size(64 * 1024),
                        )
                        .add_service(
                            VoiceSessionServer::new(voice).max_decoding_message_size(64 * 1024),
                        )
                        .serve_with_incoming_shutdown(incoming, grpc_cancel.cancelled_owned())
                        .await
                        .context("serving server gRPC")
                });
            }
            let mut failures = Vec::new();
            while let Some(result) = listeners.join_next().await {
                match result {
                    Ok(Ok(())) if cancel.is_cancelled() => {}
                    Ok(Ok(())) => {
                        failures.push("server listener exited before shutdown".to_string())
                    }
                    Ok(Err(error)) => failures.push(format!("{error:#}")),
                    Err(error) => failures.push(format!("server listener task failed: {error}")),
                }
                cancel.cancel();
            }
            anyhow::ensure!(
                failures.is_empty(),
                "server listeners failed: {}",
                failures.join("; ")
            );
            Ok(())
        });
        *service_slot = Some(task);
        drop(service_slot);
        owner_cancel.cancelled().await;
        self.request_shutdown();
        self.cleanup
            .start()
            .wait_until(self.cleanup.deadline())
            .await
    }
}

impl Drop for ServerRuntime {
    fn drop(&mut self) {
        self.request_shutdown();
        self.wiring.http.lock().expect("HTTP listener mutex").take();
        #[cfg(feature = "channel-embedded")]
        self.wiring.grpc.lock().expect("gRPC listener mutex").take();
        self.cleanup.start();
    }
}

struct HttpListener {
    listener: TcpListener,
    connections: Arc<Connections>,
}
impl axum::serve::Listener for HttpListener {
    type Io = Connection;
    type Addr = SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, addr)) => match self.connections.wrap(stream) {
                    Ok(stream) => return (stream, addr),
                    Err(_) => std::future::pending::<()>().await,
                },
                Err(error) => {
                    tracing::warn!(%error, "HTTP accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

pub(super) fn attach_frontend<S>(app: Router<S>) -> Router<S>
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
