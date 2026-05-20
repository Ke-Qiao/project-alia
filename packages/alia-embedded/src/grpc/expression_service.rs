use std::sync::Arc;

use tonic::{Request, Response, Status};

use alia_proto::expression::embedded_expression_server::EmbeddedExpression;
use alia_proto::expression::{
    CancelExpressionRequest, CancelExpressionResponse, ExecuteExpressionRequest,
    ExecuteExpressionResponse, ExpressionCapabilitiesRequest, ExpressionCapabilitiesResponse,
};

use crate::binding::BindingManager;
use crate::module::motion::MotionRuntime;

use super::auth::require_auth;

pub struct EmbeddedExpressionService {
    binding: Arc<BindingManager>,
    motion: Arc<MotionRuntime>,
}

impl EmbeddedExpressionService {
    pub fn new(binding: Arc<BindingManager>, motion: Arc<MotionRuntime>) -> Self {
        Self { binding, motion }
    }
}

#[tonic::async_trait]
impl EmbeddedExpression for EmbeddedExpressionService {
    async fn get_capabilities(
        &self,
        request: Request<ExpressionCapabilitiesRequest>,
    ) -> Result<Response<ExpressionCapabilitiesResponse>, Status> {
        require_auth(&self.binding, &request).await?;
        let capabilities_json = serde_json::to_string(&self.motion.capabilities_json().await)
            .map_err(|error| Status::internal(error.to_string()))?;
        Ok(Response::new(ExpressionCapabilitiesResponse {
            capabilities_json,
        }))
    }

    async fn execute_expression(
        &self,
        request: Request<ExecuteExpressionRequest>,
    ) -> Result<Response<ExecuteExpressionResponse>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();
        let result = self
            .motion
            .execute_expression_json(&req.plan_json)
            .await
            .map_err(|error| Status::invalid_argument(error.to_string()))?;
        let result_json =
            serde_json::to_string(&result).map_err(|error| Status::internal(error.to_string()))?;
        Ok(Response::new(ExecuteExpressionResponse { result_json }))
    }

    async fn cancel_expression(
        &self,
        request: Request<CancelExpressionRequest>,
    ) -> Result<Response<CancelExpressionResponse>, Status> {
        require_auth(&self.binding, &request).await?;
        let req = request.into_inner();
        let result = self.motion.cancel_expression_json(&req.reason).await;
        let result_json =
            serde_json::to_string(&result).map_err(|error| Status::internal(error.to_string()))?;
        Ok(Response::new(CancelExpressionResponse { result_json }))
    }
}
