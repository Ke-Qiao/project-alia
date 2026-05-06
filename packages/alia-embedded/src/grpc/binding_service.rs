use std::sync::Arc;

use tonic::{Request, Response, Status};

use alia_proto::binding::device_binding_server::DeviceBinding as DeviceBindingTrait;
use alia_proto::binding::{
    BindRequest, BindResponse, BindingStatus, CompleteHandoffRequest, HandoffRequest,
    HandoffToken, UnbindRequest, UnbindResponse,
};

use crate::binding::BindingManager;
use crate::config::IdentityConfig;

pub struct DeviceBindingService {
    binding: Arc<BindingManager>,
    identity: IdentityConfig,
}

impl DeviceBindingService {
    pub fn new(binding: Arc<BindingManager>, identity: IdentityConfig) -> Self {
        Self { binding, identity }
    }
}

#[tonic::async_trait]
impl DeviceBindingTrait for DeviceBindingService {
    async fn bind(
        &self,
        request: Request<BindRequest>,
    ) -> Result<Response<BindResponse>, Status> {
        let req = request.into_inner();
        match self
            .binding
            .bind(
                &req.server_id,
                &req.server_name,
                &req.server_endpoint,
                &req.auth_token,
            )
            .await
        {
            Ok(()) => Ok(Response::new(BindResponse {
                success: true,
                device_id: self.identity.device_id.clone(),
                device_name: self.identity.device_name.clone(),
                device_public_key: Vec::new(), // TODO: load from keypair
            })),
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
        let req = request.into_inner();
        match self.binding.unbind(&req.server_id).await {
            Ok(()) => Ok(Response::new(UnbindResponse {
                success: true,
                message: "unbound".to_string(),
            })),
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
        }))
    }

    async fn prepare_handoff(
        &self,
        request: Request<HandoffRequest>,
    ) -> Result<Response<HandoffToken>, Status> {
        let req = request.into_inner();
        match self
            .binding
            .prepare_handoff(
                &req.current_server_id,
                &req.new_server_id,
                &req.new_server_endpoint,
            )
            .await
        {
            Ok((token, expires_at)) => Ok(Response::new(HandoffToken {
                token,
                device_id: self.identity.device_id.clone(),
                expires_at,
            })),
            Err(e) => Err(Status::failed_precondition(e.to_string())),
        }
    }

    async fn complete_handoff(
        &self,
        request: Request<CompleteHandoffRequest>,
    ) -> Result<Response<BindResponse>, Status> {
        let req = request.into_inner();
        // For complete_handoff we need the new server info.
        // In production, this would be embedded in the signed token.
        // For now, return a placeholder error.
        match self
            .binding
            .complete_handoff(&req.token, "unknown", "unknown", "unknown")
            .await
        {
            Ok(()) => Ok(Response::new(BindResponse {
                success: true,
                device_id: self.identity.device_id.clone(),
                device_name: self.identity.device_name.clone(),
                device_public_key: Vec::new(),
            })),
            Err(_e) => Ok(Response::new(BindResponse {
                success: false,
                device_id: String::new(),
                device_name: String::new(),
                device_public_key: Vec::new(),
            })),
        }
    }
}
