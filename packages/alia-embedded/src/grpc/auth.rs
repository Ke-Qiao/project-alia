use tonic::{Request, Status, metadata::MetadataMap};

use alia_transport::EMBEDDED_CONTROL_SCOPE;

use crate::binding::BindingManager;
use crate::binding::BindingState;

pub async fn require_auth<T>(binding: &BindingManager, request: &Request<T>) -> Result<(), Status> {
    let token = extract_bearer_token(request.metadata())?;
    require_auth_token(binding, &token).await
}

pub async fn require_auth_token(binding: &BindingManager, token: &str) -> Result<(), Status> {
    binding
        .verify_grpc_auth(token, EMBEDDED_CONTROL_SCOPE)
        .await
        .map_err(|err| Status::unauthenticated(err.to_string()))
}

pub fn extract_bearer_token(metadata: &MetadataMap) -> Result<String, Status> {
    let raw = metadata
        .get("authorization")
        .ok_or_else(|| Status::unauthenticated("missing authorization metadata"))?;
    let raw = raw
        .to_str()
        .map_err(|_| Status::unauthenticated("invalid authorization metadata"))?;
    let token = raw
        .strip_prefix("Bearer ")
        .ok_or_else(|| Status::unauthenticated("expected Bearer token"))?;
    Ok(token.to_string())
}

pub async fn require_auth_if_bound<T>(
    binding: &BindingManager,
    request: &Request<T>,
) -> Result<(), Status> {
    let status = binding.status().await;
    if status.state == BindingState::Unbound {
        return Ok(());
    }

    let token = extract_bearer_token(request.metadata())?;
    require_auth_token(binding, &token).await
}
