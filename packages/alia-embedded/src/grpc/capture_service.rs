use std::sync::Arc;

use alia_proto::capture::{
    CaptureRequest, CaptureState, InspectCaptureRequest, capture_control_server::CaptureControl,
};
use alia_transport::EMBEDDED_CONTROL_SCOPE;
use tonic::{Request, Response, Status};

use super::auth::extract_bearer_token;
use crate::{binding::BindingManager, module::speech};

pub struct CaptureService {
    control: Arc<speech::CaptureControl>,
    binding: Arc<BindingManager>,
}

impl CaptureService {
    pub fn new(control: Arc<speech::CaptureControl>, binding: Arc<BindingManager>) -> Self {
        Self { control, binding }
    }

    async fn command(
        &self,
        request: Request<CaptureRequest>,
        start: bool,
    ) -> Result<Response<CaptureState>, Status> {
        let token = extract_bearer_token(request.metadata())?;
        let request = request.into_inner();
        let boot = uuid::Uuid::parse_str(&request.device_boot_id)
            .map_err(|_| Status::invalid_argument("device_boot_id must be a UUID"))?;
        if boot.is_nil() {
            return Err(Status::invalid_argument("device_boot_id must not be nil"));
        }
        // Pin before admission. A replacement in either gap is rejected again
        // by the runtime under the original binding's read lock.
        let original = self.binding.status().await;
        let response = self
            .binding
            .with_verified_grpc_auth(&token, EMBEDDED_CONTROL_SCOPE, || {
                Ok(self
                    .control
                    .enqueue(request, start, original, token.clone()))
            })
            .await
            .map_err(|error| Status::unauthenticated(error.to_string()))?
            .map_err(|error| Status::failed_precondition(error.to_string()))?;
        let result = response
            .await
            .map_err(|_| Status::unavailable("capture pipeline stopped before acknowledgement"))?
            .map_err(|error| Status::failed_precondition(error.to_string()))?;
        Ok(Response::new(result))
    }
}

#[tonic::async_trait]
impl CaptureControl for CaptureService {
    async fn inspect(
        &self,
        request: Request<InspectCaptureRequest>,
    ) -> Result<Response<CaptureState>, Status> {
        let token = extract_bearer_token(request.metadata())?;
        let state = self
            .binding
            .with_verified_grpc_auth(&token, EMBEDDED_CONTROL_SCOPE, || {
                Ok(self.control.inspect())
            })
            .await
            .map_err(|error| Status::unauthenticated(error.to_string()))?
            .map_err(|error| Status::internal(error.to_string()))?;
        Ok(Response::new(state))
    }

    async fn start(
        &self,
        request: Request<CaptureRequest>,
    ) -> Result<Response<CaptureState>, Status> {
        self.command(request, true).await
    }

    async fn stop(
        &self,
        request: Request<CaptureRequest>,
    ) -> Result<Response<CaptureState>, Status> {
        self.command(request, false).await
    }
}
