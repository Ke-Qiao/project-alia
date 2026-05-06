use std::sync::Arc;

use tonic::{Request, Response, Status};

use alia_proto::plugin::plugin_manager_server::PluginManager as PluginManagerTrait;
use alia_proto::plugin::{
    InstallPluginRequest, InstallPluginResponse, ListPluginsRequest, ListPluginsResponse,
    PluginCommandRequest, PluginCommandResponse, PluginEvent, PluginInfo,
    StartPluginRequest, StartPluginResponse, StopPluginRequest, StopPluginResponse,
    StreamEventsRequest, UninstallPluginRequest, UninstallPluginResponse,
};

use crate::plugin::{PluginRegistry, PluginState};

pub struct PluginManagerService {
    registry: Arc<PluginRegistry>,
}

impl PluginManagerService {
    pub fn new(registry: Arc<PluginRegistry>) -> Self {
        Self { registry }
    }
}

#[tonic::async_trait]
impl PluginManagerTrait for PluginManagerService {
    async fn install_plugin(
        &self,
        request: Request<InstallPluginRequest>,
    ) -> Result<Response<InstallPluginResponse>, Status> {
        let req = request.into_inner();
        match self.registry.install(&req.name, &req.zip_data).await {
            Ok(()) => Ok(Response::new(InstallPluginResponse {
                success: true,
                message: "installed".to_string(),
            })),
            Err(e) => Ok(Response::new(InstallPluginResponse {
                success: false,
                message: e.to_string(),
            })),
        }
    }

    async fn uninstall_plugin(
        &self,
        request: Request<UninstallPluginRequest>,
    ) -> Result<Response<UninstallPluginResponse>, Status> {
        let req = request.into_inner();
        match self.registry.uninstall(&req.name).await {
            Ok(()) => Ok(Response::new(UninstallPluginResponse {
                success: true,
                message: "uninstalled".to_string(),
            })),
            Err(e) => Ok(Response::new(UninstallPluginResponse {
                success: false,
                message: e.to_string(),
            })),
        }
    }

    async fn list_plugins(
        &self,
        _request: Request<ListPluginsRequest>,
    ) -> Result<Response<ListPluginsResponse>, Status> {
        let snapshots = self.registry.list().await;
        let plugins = snapshots
            .into_iter()
            .map(|s| PluginInfo {
                name: s.name,
                version: s.version,
                plugin_type: s.plugin_type,
                state: plugin_state_to_proto(s.state) as i32,
                description: s.description,
            })
            .collect();

        Ok(Response::new(ListPluginsResponse { plugins }))
    }

    async fn start_plugin(
        &self,
        request: Request<StartPluginRequest>,
    ) -> Result<Response<StartPluginResponse>, Status> {
        // TODO: integrate with ProcessManager to start plugin subprocess
        let _req = request.into_inner();
        Ok(Response::new(StartPluginResponse {
            success: false,
            message: "not yet implemented".to_string(),
        }))
    }

    async fn stop_plugin(
        &self,
        request: Request<StopPluginRequest>,
    ) -> Result<Response<StopPluginResponse>, Status> {
        // TODO: integrate with ProcessManager to stop plugin subprocess
        let _req = request.into_inner();
        Ok(Response::new(StopPluginResponse {
            success: false,
            message: "not yet implemented".to_string(),
        }))
    }

    async fn send_command(
        &self,
        request: Request<PluginCommandRequest>,
    ) -> Result<Response<PluginCommandResponse>, Status> {
        // TODO: route command to plugin's stdin
        let _req = request.into_inner();
        Ok(Response::new(PluginCommandResponse {
            success: false,
            message: "not yet implemented".to_string(),
        }))
    }

    type StreamEventsStream =
        tokio_stream::wrappers::ReceiverStream<Result<PluginEvent, Status>>;

    async fn stream_events(
        &self,
        _request: Request<StreamEventsRequest>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        // TODO: stream plugin stdout events
        let (_tx, rx) = tokio::sync::mpsc::channel(128);
        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
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
