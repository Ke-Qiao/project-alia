use std::sync::Arc;

use tonic::{Request, Response, Status};

use alia_proto::binding::device_binding_server::DeviceBinding as DeviceBindingTrait;
use alia_proto::binding::{
    BindRequest, BindResponse, BindingStatus, CompleteHandoffRequest, HandoffRequest, HandoffToken,
    UnbindRequest, UnbindResponse,
};

use super::auth::require_auth;
use crate::binding::BindingManager;
use crate::config::IdentityConfig;
use crate::module::traits::DiscoveryBackend;
use crate::plugin::PluginRegistry;
use crate::settings::EmbeddedSettingsManager;

pub struct DeviceBindingService {
    binding: Arc<BindingManager>,
    identity: IdentityConfig,
    discovery: Arc<dyn DiscoveryBackend>,
    plugins: Arc<PluginRegistry>,
    settings: Arc<EmbeddedSettingsManager>,
    public_key: Vec<u8>,
}

impl DeviceBindingService {
    pub fn new(
        binding: Arc<BindingManager>,
        identity: IdentityConfig,
        discovery: Arc<dyn DiscoveryBackend>,
        plugins: Arc<PluginRegistry>,
        settings: Arc<EmbeddedSettingsManager>,
        public_key: Vec<u8>,
    ) -> Self {
        Self {
            binding,
            identity,
            discovery,
            plugins,
            settings,
            public_key,
        }
    }

    async fn refresh_discovery_metadata(&self) {
        let mut metadata = self.binding.status().await.discovery_metadata();
        metadata.attributes.insert(
            "provisioning_mode".to_string(),
            self.settings.provisioning_mode().await.to_string(),
        );
        self.plugins.enrich_discovery_metadata(&mut metadata).await;
        if let Err(error) = self.discovery.update_metadata(&metadata).await {
            tracing::warn!(error = %error, "failed to refresh discovery metadata");
        }
    }
}

#[tonic::async_trait]
impl DeviceBindingTrait for DeviceBindingService {
    async fn bind(&self, request: Request<BindRequest>) -> Result<Response<BindResponse>, Status> {
        let req = request.into_inner();
        match self
            .binding
            .bind(
                &req.server_id,
                &req.server_name,
                &req.server_endpoint,
                &req.server_public_key,
                &req.auth_token,
                req.issued_at,
            )
            .await
        {
            Ok(()) => {
                self.refresh_discovery_metadata().await;
                Ok(Response::new(BindResponse {
                    success: true,
                    device_id: self.identity.device_id.clone(),
                    device_name: self.identity.device_name.clone(),
                    device_public_key: self.public_key.clone(),
                }))
            }
            Err(_e) => Ok(Response::new(BindResponse {
                success: false,
                device_id: String::new(),
                device_name: String::new(),
                device_public_key: Vec::new(),
            })),
        }
    }

    async fn unbind(
        &self,
        request: Request<UnbindRequest>,
    ) -> Result<Response<UnbindResponse>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();
        match self.binding.unbind(&req.server_id).await {
            Ok(()) => {
                self.refresh_discovery_metadata().await;
                Ok(Response::new(UnbindResponse {
                    success: true,
                    message: "unbound".to_string(),
                }))
            }
            Err(e) => Ok(Response::new(UnbindResponse {
                success: false,
                message: e.to_string(),
            })),
        }
    }

    async fn get_binding_status(
        &self,
        _request: Request<()>,
    ) -> Result<Response<BindingStatus>, Status> {
        let data = self.binding.status().await;
        Ok(Response::new(BindingStatus {
            is_bound: data.state == crate::binding::BindingState::Bound,
            bound_server_id: data.server_id.unwrap_or_default(),
            bound_server_name: data.server_name.unwrap_or_default(),
            bound_since: data.bound_since.unwrap_or_default(),
            state: match data.state {
                crate::binding::BindingState::Unbound => 0,
                crate::binding::BindingState::Bound => 1,
                crate::binding::BindingState::HandoffPending => 2,
            },
            device_id: self.identity.device_id.clone(),
            device_public_key: self.public_key.clone(),
        }))
    }

    async fn prepare_handoff(
        &self,
        request: Request<HandoffRequest>,
    ) -> Result<Response<HandoffToken>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();
        match self
            .binding
            .prepare_handoff(
                &req.current_server_id,
                &req.new_server_id,
                &req.new_server_name,
                &req.new_server_endpoint,
                &req.new_server_public_key,
                &req.authorization,
                req.issued_at,
            )
            .await
        {
            Ok((token, expires_at)) => {
                self.refresh_discovery_metadata().await;
                Ok(Response::new(HandoffToken {
                    token,
                    device_id: self.identity.device_id.clone(),
                    expires_at,
                }))
            }
            Err(e) => Err(Status::failed_precondition(e.to_string())),
        }
    }

    async fn complete_handoff(
        &self,
        request: Request<CompleteHandoffRequest>,
    ) -> Result<Response<BindResponse>, Status> {
        let req = request.into_inner();
        match self
            .binding
            .complete_handoff(
                &req.token,
                &req.new_server_id,
                &req.new_server_name,
                &req.new_server_endpoint,
                &req.new_server_public_key,
                &req.authorization,
                req.issued_at,
            )
            .await
        {
            Ok(()) => {
                self.refresh_discovery_metadata().await;
                Ok(Response::new(BindResponse {
                    success: true,
                    device_id: self.identity.device_id.clone(),
                    device_name: self.identity.device_name.clone(),
                    device_public_key: self.public_key.clone(),
                }))
            }
            Err(_e) => Ok(Response::new(BindResponse {
                success: false,
                device_id: String::new(),
                device_name: String::new(),
                device_public_key: Vec::new(),
            })),
        }
    }
}
