use alia_transport::{DEFAULT_GRPC_AUTH_TTL, mint_auth_token};
use ed25519_dalek::SigningKey;
use tonic::Request;

pub(super) fn auth_request_with_signing_key<T>(
    signing_key: &SigningKey,
    server_id: &str,
    message: T,
) -> anyhow::Result<Request<T>> {
    let token = mint_auth_token(
        signing_key,
        server_id,
        "embedded:control",
        DEFAULT_GRPC_AUTH_TTL,
    )?;
    let mut request = Request::new(message);
    let header = format!("Bearer {token}")
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid authorization metadata: {e}"))?;
    request.metadata_mut().insert("authorization", header);
    Ok(request)
}
