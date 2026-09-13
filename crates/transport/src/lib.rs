pub mod connections;

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use serde::{Deserialize, Serialize};

pub const DEFAULT_EMBEDDED_DISCOVERY_SERVICE: &str = "_alia-embedded._tcp.local.";
pub const DEFAULT_AUTH_MAX_SKEW: Duration = Duration::from_secs(300);
pub const DEFAULT_GRPC_AUTH_TTL: Duration = Duration::from_secs(15 * 60);
pub const EMBEDDED_CONTROL_SCOPE: &str = "embedded:control";
pub const EMBEDDED_INGRESS_SCOPE: &str = "embedded:ingress";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveredService {
    pub endpoint: String,
    pub instance_name: String,
    pub hostname: String,
    pub port: u16,
    pub properties: HashMap<String, String>,
}

pub async fn discover_embedded_endpoint(
    service_name: &str,
    timeout: Duration,
) -> Result<Option<String>> {
    discover_service_endpoint(service_name, timeout).await
}

pub async fn discover_service_endpoint(
    service_name: &str,
    timeout: Duration,
) -> Result<Option<String>> {
    Ok(discover_service(service_name, timeout)
        .await?
        .map(|service| service.endpoint))
}

pub async fn discover_service(
    service_name: &str,
    timeout: Duration,
) -> Result<Option<DiscoveredService>> {
    discover_service_cancellable(
        service_name,
        timeout,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

/// The worker owns the native daemon through its shutdown acknowledgement.
/// Dropping this waiter requests cancellation without aborting that owner.
/// Callers that own a runtime loop should cancel the token and keep awaiting to
/// observe cleanup; the hosting Tokio runtime must remain alive during cleanup.
pub async fn discover_service_cancellable(
    service_name: &str,
    timeout: Duration,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<Option<DiscoveredService>> {
    let service_name = normalize_service_name(service_name);
    let cancel = cancellation.child_token();
    let worker_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        if worker_cancel.is_cancelled() {
            return Ok(None);
        }
        let result = discover_owned(service_name, timeout, worker_cancel).await;
        if let Err(error) = &result {
            // The waiter may have been dropped; retain visible failure evidence.
            tracing::warn!(error = %error, "mDNS discovery owner failed");
        }
        result
    });
    await_discovery(task, cancel).await
}

async fn await_discovery(
    task: tokio::task::JoinHandle<Result<Option<DiscoveredService>>>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<Option<DiscoveredService>> {
    let _cancel_on_drop = cancel.drop_guard();
    task.await.context("joining mDNS discovery owner")?
}

async fn discover_owned(
    service_name: String,
    timeout: Duration,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<Option<DiscoveredService>> {
    let mdns = ServiceDaemon::new().context("creating mDNS discovery daemon")?;
    let result = async {
        let receiver = mdns.browse(&service_name)
            .with_context(|| format!("browsing for {service_name}"))?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Ok(None),
            _ = tokio::time::sleep(timeout) => Ok(None),
            result = async {
                loop {
                    match receiver.recv_async().await {
                        Ok(ServiceEvent::ServiceResolved(info)) => {
                            if let Some(service) = resolved_service(&info) { return Ok(Some(service)); }
                        }
                        Ok(_) => {},
                        Err(error) => return Err(anyhow::anyhow!("mDNS receiver closed: {error}")),
                    }
                }
            } => result,
        }
    }.await;
    let closed = shutdown_discovery_daemon(&mdns).await;
    match (result, closed) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("mDNS shutdown also failed: {cleanup:#}")))
        }
    }
}

/// Wait for the native library's shutdown evidence. This is not an OS-thread
/// JoinHandle: the library exposes a daemon acknowledgement, not thread join.
pub async fn shutdown_discovery_daemon(mdns: &ServiceDaemon) -> Result<()> {
    let receiver = loop {
        match mdns.shutdown() {
            Ok(receiver) => break receiver,
            Err(mdns_sd::Error::Again) => tokio::time::sleep(Duration::from_millis(10)).await,
            Err(error) => return Err(error).context("requesting mDNS daemon shutdown"),
        }
    };
    let status = receiver
        .recv_async()
        .await
        .context("waiting for mDNS daemon shutdown")?;
    anyhow::ensure!(
        status == mdns_sd::DaemonStatus::Shutdown,
        "unexpected mDNS shutdown status: {status:?}"
    );
    Ok(())
}

pub fn current_unix_timestamp() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before unix epoch")?
        .as_secs())
}

pub fn ensure_recent_timestamp(issued_at: u64, max_skew: Duration) -> Result<()> {
    let now = current_unix_timestamp()?;
    let delta = now.abs_diff(issued_at);
    if delta > max_skew.as_secs() {
        anyhow::bail!("timestamp skew too large");
    }
    Ok(())
}

pub fn bind_payload(
    server_id: &str,
    server_name: &str,
    server_endpoint: &str,
    server_public_key: &[u8],
    issued_at: u64,
) -> Vec<u8> {
    framed_payload(&[
        b"alia-bind-v1".as_slice(),
        server_id.as_bytes(),
        server_name.as_bytes(),
        server_endpoint.as_bytes(),
        server_public_key,
        &issued_at.to_be_bytes(),
    ])
}

pub fn handoff_intent_payload(
    current_server_id: &str,
    new_server_id: &str,
    new_server_name: &str,
    new_server_endpoint: &str,
    new_server_public_key: &[u8],
    issued_at: u64,
) -> Vec<u8> {
    framed_payload(&[
        b"alia-handoff-intent-v1".as_slice(),
        current_server_id.as_bytes(),
        new_server_id.as_bytes(),
        new_server_name.as_bytes(),
        new_server_endpoint.as_bytes(),
        new_server_public_key,
        &issued_at.to_be_bytes(),
    ])
}

pub fn handoff_completion_payload(
    token: &[u8],
    new_server_id: &str,
    new_server_name: &str,
    new_server_endpoint: &str,
    new_server_public_key: &[u8],
    issued_at: u64,
) -> Vec<u8> {
    framed_payload(&[
        b"alia-handoff-complete-v1".as_slice(),
        token,
        new_server_id.as_bytes(),
        new_server_name.as_bytes(),
        new_server_endpoint.as_bytes(),
        new_server_public_key,
        &issued_at.to_be_bytes(),
    ])
}

pub fn sign_payload(signing_key: &SigningKey, payload: &[u8]) -> Vec<u8> {
    signing_key.sign(payload).to_bytes().to_vec()
}

/// Sign the exact bounded body request bytes. A request signature cannot be
/// reused as a device receipt, a bearer token or another control protocol.
pub fn body_request_payload(payload: &[u8]) -> Vec<u8> {
    framed_payload(&[b"alia-body-v1-request", payload])
}

pub fn body_reply_payload(payload: &[u8]) -> Vec<u8> {
    framed_payload(&[b"alia-body-v1-reply", payload])
}

pub fn verify_payload(public_key: &[u8], payload: &[u8], signature: &[u8]) -> Result<()> {
    let public_key: [u8; 32] = public_key
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid Ed25519 public key length"))?;
    let signature: [u8; 64] = signature
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid Ed25519 signature length"))?;

    let verifying_key =
        VerifyingKey::from_bytes(&public_key).context("parsing Ed25519 verifying key")?;
    let signature = Signature::from_bytes(&signature);
    verifying_key
        .verify(payload, &signature)
        .context("signature verification failed")?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokenClaims {
    pub version: u8,
    pub server_id: String,
    pub scope: String,
    pub iat: u64,
    pub exp: u64,
}

pub fn mint_auth_token(
    signing_key: &SigningKey,
    server_id: &str,
    scope: &str,
    ttl: Duration,
) -> Result<String> {
    let issued_at = current_unix_timestamp()?;
    let claims = AuthTokenClaims {
        version: 1,
        server_id: server_id.to_string(),
        scope: scope.to_string(),
        iat: issued_at,
        exp: issued_at + ttl.as_secs(),
    };
    let payload = serde_json::to_vec(&claims).context("serializing auth token claims")?;
    let signature = sign_payload(signing_key, &payload);
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    Ok(format!(
        "{}.{}",
        engine.encode(payload),
        engine.encode(signature)
    ))
}

pub fn verify_auth_token(
    token: &str,
    public_key: &[u8],
    expected_server_id: &str,
    required_scope: &str,
    max_skew: Duration,
) -> Result<AuthTokenClaims> {
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let (payload_b64, sig_b64) = token
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("invalid auth token format"))?;
    let payload = engine
        .decode(payload_b64)
        .context("decoding auth token payload")?;
    let signature = engine
        .decode(sig_b64)
        .context("decoding auth token signature")?;
    verify_payload(public_key, &payload, &signature)?;

    let claims: AuthTokenClaims =
        serde_json::from_slice(&payload).context("parsing auth token claims")?;
    if claims.version != 1 {
        anyhow::bail!("unsupported auth token version");
    }
    if claims.server_id != expected_server_id {
        anyhow::bail!("auth token server_id mismatch");
    }
    if claims.scope != required_scope {
        anyhow::bail!("auth token scope mismatch");
    }
    ensure_recent_timestamp(claims.iat, max_skew)?;
    let now = current_unix_timestamp()?;
    if now > claims.exp {
        anyhow::bail!("auth token expired");
    }
    Ok(claims)
}

pub fn embedded_device_auth_subject(device_id: &str, device_public_key: &[u8]) -> String {
    let device_id = device_id.trim();
    if !device_id.is_empty() {
        return device_id.to_string();
    }

    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(device_public_key)
}

fn framed_payload(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(&(part.len() as u32).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

pub fn normalize_service_name(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('.');
    if trimmed.ends_with(".local") {
        format!("{trimmed}.")
    } else {
        format!("{trimmed}.local.")
    }
}

fn resolved_service_endpoint(info: &ResolvedService) -> Option<String> {
    let address = info
        .get_addresses_v4()
        .into_iter()
        .map(IpAddr::V4)
        .next()
        .or_else(|| {
            info.get_addresses()
                .iter()
                .next()
                .map(|scoped| scoped.to_ip_addr())
        })?;

    Some(match address {
        IpAddr::V4(ip) => format!("http://{ip}:{}", info.get_port()),
        IpAddr::V6(ip) => format!("http://[{ip}]:{}", info.get_port()),
    })
}

fn resolved_service(info: &ResolvedService) -> Option<DiscoveredService> {
    let endpoint = resolved_service_endpoint(info)?;
    let properties = info
        .get_properties()
        .iter()
        .map(|property| (property.key().to_string(), property.val_str().to_string()))
        .collect::<HashMap<_, _>>();

    Some(DiscoveredService {
        endpoint,
        instance_name: info.get_fullname().to_string(),
        hostname: info.get_hostname().to_string(),
        port: info.get_port(),
        properties,
    })
}

#[cfg(test)]
mod discovery_lifecycle_tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_discovery_waiter_preserves_owner_through_native_shutdown_ack() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let owner_cancel = cancel.clone();
        let (completed, completion) = tokio::sync::oneshot::channel();
        let owner = tokio::spawn(async move {
            let result = discover_owned(
                "_alia-test-drop._tcp.local.".into(),
                Duration::from_secs(60),
                owner_cancel,
            )
            .await;
            // discover_owned returns only after the actual daemon shutdown ACK.
            let _ = completed.send(result);
            Ok(None)
        });
        let waiter = tokio::spawn(await_discovery(owner, cancel.clone()));
        tokio::time::sleep(Duration::from_millis(50)).await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(cancel.is_cancelled());
        assert!(
            tokio::time::timeout(Duration::from_secs(2), completion)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .is_none()
        );
    }
}
