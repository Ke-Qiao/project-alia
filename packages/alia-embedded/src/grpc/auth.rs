use tonic::{Request, Status, metadata::MetadataMap};

use crate::binding::BindingManager;
use crate::binding::BindingState;

pub const EMBEDDED_CONTROL_SCOPE: &str = "embedded:control";

pub async fn require_auth<T>(binding: &BindingManager, request: &Request<T>) -> Result<(), Status> {
    let token = bearer_token(request.metadata())?;
    binding
        .verify_grpc_auth(&token, EMBEDDED_CONTROL_SCOPE)
        .await
        .map_err(|err| Status::unauthenticated(err.to_string()))
}

pub async fn require_auth_if_bound<T>(
    binding: &BindingManager,
    request: &Request<T>,
) -> Result<(), Status> {
    let status = binding.status().await;
    if status.state == BindingState::Unbound {
        return Ok(());
    }

    require_auth(binding, request).await
}

fn bearer_token(metadata: &MetadataMap) -> Result<String, Status> {
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
