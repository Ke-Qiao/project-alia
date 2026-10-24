use std::sync::Arc;

use tonic::{Request, Response, Status};

use alia_proto::plugin::plugin_manager_server::PluginManager as PluginManagerTrait;
use alia_proto::plugin::{
    InstallPluginRequest, InstallPluginResponse, ListPluginsRequest, ListPluginsResponse,
    PluginCommandRequest, PluginCommandResponse, PluginEvent, PluginInfo, StartPluginRequest,
    StartPluginResponse, StopPluginRequest, StopPluginResponse, StreamEventsRequest,
    UninstallPluginRequest, UninstallPluginResponse,
};

use super::auth::require_auth;
use crate::module::motion::MotionRuntime;
use crate::plugin::{PluginRegistry, PluginState};
use crate::settings::EmbeddedSettingsManager;
use crate::{binding::BindingManager, module::traits::DiscoveryBackend};

pub struct PluginManagerService {
    registry: Arc<PluginRegistry>,
    binding: Arc<BindingManager>,
    discovery: Arc<dyn DiscoveryBackend>,
    motion: Arc<MotionRuntime>,
    settings: Arc<EmbeddedSettingsManager>,
}

impl PluginManagerService {
    pub fn new(
        registry: Arc<PluginRegistry>,
        binding: Arc<BindingManager>,
        discovery: Arc<dyn DiscoveryBackend>,
        motion: Arc<MotionRuntime>,
        settings: Arc<EmbeddedSettingsManager>,
    ) -> Self {
        Self {
            registry,
            binding,
            discovery,
            motion,
            settings,
        }
    }

    async fn refresh_discovery_metadata(&self) {
        let mut metadata = self.binding.status().await.discovery_metadata();
        metadata.attributes.insert(
            "provisioning_mode".to_string(),
            self.settings.provisioning_mode().await.to_string(),
        );
        self.motion.enrich_discovery_metadata(&mut metadata);
        self.registry.enrich_discovery_metadata(&mut metadata).await;
        if let Err(error) = self.discovery.update_metadata(&metadata).await {
            tracing::warn!(
                error = %error,
                "failed to refresh discovery metadata after plugin change"
            );
        }
    }
}

#[tonic::async_trait]
impl PluginManagerTrait for PluginManagerService {
    async fn install_plugin(
        &self,
        request: Request<InstallPluginRequest>,
    ) -> Result<Response<InstallPluginResponse>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();
        match self
            .registry
            .install(&req.name, &req.version, &req.zip_data)
            .await
        {
            Ok(asset_sha256) => {
                self.refresh_discovery_metadata().await;
                Ok(Response::new(InstallPluginResponse {
                    success: true,
                    message: "installed".to_string(),
                    asset_sha256,
                }))
            }
            Err(e) => Ok(Response::new(InstallPluginResponse {
                success: false,
                message: e.to_string(),
                asset_sha256: String::new(),
            })),
        }
    }

    async fn uninstall_plugin(
        &self,
        request: Request<UninstallPluginRequest>,
    ) -> Result<Response<UninstallPluginResponse>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();
        match self.registry.uninstall(&req.name).await {
            Ok(()) => {
                self.refresh_discovery_metadata().await;
                Ok(Response::new(UninstallPluginResponse {
                    success: true,
                    message: "uninstalled".to_string(),
                }))
            }
            Err(e) => Ok(Response::new(UninstallPluginResponse {
                success: false,
                message: e.to_string(),
            })),
        }
    }

    async fn list_plugins(
        &self,
        request: Request<ListPluginsRequest>,
    ) -> Result<Response<ListPluginsResponse>, Status> {
        require_auth(&self.binding, &request).await?;
        let snapshots = self.registry.list().await;
        let plugins = snapshots
            .into_iter()
            .map(|s| PluginInfo {
                name: s.name,
                version: s.version,
                plugin_type: s.plugin_type,
                state: plugin_state_to_proto(s.state) as i32,
                description: s.description,
                capabilities: s.capabilities,
                asset_sha256: s.asset_sha256,
                instance_id: s.instance_id,
            })
            .collect();

        Ok(Response::new(ListPluginsResponse { plugins }))
    }

    async fn start_plugin(
        &self,
        request: Request<StartPluginRequest>,
    ) -> Result<Response<StartPluginResponse>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();
        match self
            .registry
            .start(
                &req.name,
                &req.expected_asset_sha256,
                req.allow_trusted_native,
            )
            .await
        {
            Ok(()) => {
                self.refresh_discovery_metadata().await;
                Ok(Response::new(StartPluginResponse {
                    success: true,
                    message: "activation admitted; current state is reported by ListPlugins"
                        .to_string(),
                }))
            }
            Err(e) => Ok(Response::new(StartPluginResponse {
                success: false,
                message: e.to_string(),
            })),
        }
    }

    async fn stop_plugin(
        &self,
        request: Request<StopPluginRequest>,
    ) -> Result<Response<StopPluginResponse>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();
        match self.registry.stop(&req.name).await {
            Ok(()) => {
                self.refresh_discovery_metadata().await;
                Ok(Response::new(StopPluginResponse {
                    success: true,
                    message: "stopping".to_string(),
                }))
            }
            Err(e) => Ok(Response::new(StopPluginResponse {
                success: false,
                message: e.to_string(),
            })),
        }
    }

    async fn send_command(
        &self,
        request: Request<PluginCommandRequest>,
    ) -> Result<Response<PluginCommandResponse>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();
        match self
            .registry
            .send_command(
                &req.plugin_name,
                &req.instance_id,
                &req.command_id,
                &req.action,
                &req.payload_json,
            )
            .await
        {
            Ok(receipt) => Ok(Response::new(PluginCommandResponse {
                success: receipt.state == "completed",
                message: receipt.state.clone(),
                state: receipt.state,
                instance_id: receipt.instance_id,
                command_id: receipt.command_id,
                result_json: receipt.result_json,
            })),
            Err(e) => Ok(Response::new(PluginCommandResponse {
                success: false,
                message: e.to_string(),
                state: "rejected".into(),
                instance_id: req.instance_id,
                command_id: req.command_id,
                result_json: Vec::new(),
            })),
        }
    }

    type StreamEventsStream = tokio_stream::wrappers::ReceiverStream<Result<PluginEvent, Status>>;

    async fn stream_events(
        &self,
        request: Request<StreamEventsRequest>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();
        let mut rx = self.registry.subscribe_events();
        let plugin_filter = req.plugin_names;
        let event_filter = req.event_types;
        let (tx, stream_rx) = tokio::sync::mpsc::channel(128);

        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = tx.closed() => break,
                    event = rx.recv() => match event {
                        Ok(event) => event,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            let _ = tx.send(Err(Status::data_loss(format!("plugin event stream lost {skipped} events; query the original command identities")))).await;
                            break;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                };
                if !plugin_filter.is_empty() && !plugin_filter.contains(&event.plugin_name) {
                    continue;
                }
                if !event_filter.is_empty() && !event_filter.contains(&event.event_type) {
                    continue;
                }

                let message = PluginEvent {
                    plugin_name: event.plugin_name,
                    event_type: event.event_type,
                    payload_json: event.payload_json,
                    timestamp_us: event.timestamp_us,
                    instance_id: event.instance_id,
                    command_id: event.command_id,
                };

                if tx.send(Ok(message)).await.is_err() {
                    break;
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            stream_rx,
        )))
    }
}

fn plugin_state_to_proto(state: PluginState) -> i32 {
    match state {
        PluginState::Installed => 1,
        PluginState::Starting => 2,
        PluginState::Running => 3,
        PluginState::Stopping => 4,
        PluginState::Stopped => 5,
        PluginState::Failed => 6,
    }
}
