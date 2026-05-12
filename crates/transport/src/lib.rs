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
    let service_name = normalize_service_name(service_name);
    let mdns = ServiceDaemon::new().context("creating mDNS discovery daemon")?;
    let receiver = mdns
        .browse(&service_name)
        .with_context(|| format!("browsing for {service_name}"))?;

    let result = tokio::time::timeout(timeout, async move {
        loop {
            match receiver.recv_async().await {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    if let Some(service) = resolved_service(&info) {
                        return Ok(Some(service));
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    return Err(anyhow::anyhow!("mDNS receiver closed: {error}"));
                }
            }
        }
    })
    .await;

    let _ = mdns.shutdown();

    match result {
        Ok(found) => found,
        Err(_) => Ok(None),
    }
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

fn framed_payload(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(&(part.len() as u32).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

fn normalize_service_name(raw: &str) -> String {
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
mod tests {
    use super::*;

    #[test]
    fn normalizes_embedded_service_name() {
        assert_eq!(
            normalize_service_name("_alia-embedded._tcp"),
            "_alia-embedded._tcp.local."
        );
        assert_eq!(
            normalize_service_name(DEFAULT_EMBEDDED_DISCOVERY_SERVICE),
            "_alia-embedded._tcp.local."
        );
    }

    #[test]
    fn signs_and_verifies_payload() {
        let secret: [u8; 32] = rand::random();
        let signing_key = SigningKey::from_bytes(&secret);
        let payload = bind_payload("server-1", "Server", "http://127.0.0.1:3000", &[], 42);
        let signature = sign_payload(&signing_key, &payload);
        let public_key = signing_key.verifying_key().to_bytes();
        verify_payload(&public_key, &payload, &signature).unwrap();
    }

    #[test]
    fn mints_and_verifies_auth_token() {
        let secret: [u8; 32] = rand::random();
        let signing_key = SigningKey::from_bytes(&secret);
        let public_key = signing_key.verifying_key().to_bytes();
        let token = mint_auth_token(
            &signing_key,
            "server-main",
            "embedded:control",
            Duration::from_secs(900),
        )
        .unwrap();
        let claims = verify_auth_token(
            &token,
            &public_key,
            "server-main",
            "embedded:control",
            Duration::from_secs(300),
        )
        .unwrap();
        assert_eq!(claims.server_id, "server-main");
    }
}
