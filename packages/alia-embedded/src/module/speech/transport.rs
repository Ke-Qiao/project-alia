use std::path::Path;

use alia_transport::{
    DEFAULT_GRPC_AUTH_TTL, EMBEDDED_INGRESS_SCOPE, embedded_device_auth_subject, mint_auth_token,
};
use anyhow::{Context, Result};

use crate::binding::BindingData;
use crate::identity;

pub(super) fn voice_connection(
    original: &BindingData,
    device_id: &str,
    keypair_path: &Path,
) -> Result<(String, VoiceAuth)> {
    anyhow::ensure!(
        original.state == crate::binding::BindingState::Bound,
        "device has no active binding"
    );
    let endpoint = original
        .server_endpoint
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("cannot open voice session before device is bound"))?;
    Ok((
        derive_voice_endpoint(endpoint)?,
        voice_auth(device_id, keypair_path)?,
    ))
}

pub fn derive_voice_endpoint(server_endpoint: &str) -> Result<String> {
    let (scheme, rest) = server_endpoint
        .split_once("://")
        .ok_or_else(|| anyhow::anyhow!("server endpoint must include scheme"))?;
    let (authority, _path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("server endpoint must include host:port"))?;
    let port = port
        .parse::<u16>()
        .context("parsing server endpoint port")?;
    let voice_port = port
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("server endpoint port cannot be incremented"))?;
    Ok(format!("{scheme}://{host}:{voice_port}"))
}

#[derive(Debug, Clone)]
pub(super) struct VoiceAuth {
    pub(super) token: String,
}

pub(super) fn voice_auth(device_id: &str, keypair_path: &Path) -> Result<VoiceAuth> {
    let keypair = identity::load_device_keypair(keypair_path)?;
    let public_key = keypair.public_key_bytes();
    let subject = embedded_device_auth_subject(device_id, &public_key);
    let token = mint_auth_token(
        keypair.signing_key(),
        &subject,
        EMBEDDED_INGRESS_SCOPE,
        DEFAULT_GRPC_AUTH_TTL,
    )?;
    Ok(VoiceAuth { token })
}

pub(super) fn attach_voice_auth(
    metadata: &mut tonic::metadata::MetadataMap,
    auth: &VoiceAuth,
    channel_id: &str,
) -> Result<()> {
    crate::config::validate_speech_channel_id(channel_id)?;
    let channel = channel_id.parse().context("building channel id metadata")?;
    let authorization = format!("Bearer {}", auth.token)
        .parse()
        .context("building authorization metadata")?;
    metadata.insert("authorization", authorization);

    metadata.insert("x-alia-channel-id", channel);
    Ok(())
}

pub(super) fn pcm16_to_le_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}
