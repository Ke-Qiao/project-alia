use std::path::Path;
use std::sync::Arc;

use alia_proto::audio::server_audio_ingress_client::ServerAudioIngressClient;
use alia_proto::audio::{
    AudioChunk, AudioEncoding, CapturedAudioChunk, InterruptActiveTurnRequest,
};
use alia_transport::{
    DEFAULT_GRPC_AUTH_TTL, EMBEDDED_INGRESS_SCOPE, embedded_device_auth_subject, mint_auth_token,
};
use anyhow::{Context, Result};
use futures::stream;
use tracing::{debug, warn};

use crate::binding::{BindingData, BindingManager};
use crate::identity;

use super::capture::SpeechCapture;

pub(super) fn voice_connection(
    original: &BindingData,
    device_id: &str,
    keypair_path: &Path,
) -> Result<(String, IngressAuth)> {
    anyhow::ensure!(
        original.state == crate::binding::BindingState::Bound,
        "device has no active binding"
    );
    let endpoint = original
        .server_endpoint
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("cannot open voice session before device is bound"))?;
    Ok((
        derive_audio_ingress_endpoint(endpoint)?,
        ingress_auth(device_id, keypair_path)?,
    ))
}

pub(super) async fn upload_to_server(
    capture: &SpeechCapture,
    binding: &BindingManager,
    device_id: &str,
    keypair_path: &Path,
) -> bool {
    let status = binding.status().await;
    let Some(server_endpoint) = status.server_endpoint.as_deref() else {
        warn!("cannot upload speech to server before device is bound");
        return false;
    };
    let endpoint = match derive_audio_ingress_endpoint(server_endpoint) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            warn!(error = %error, server_endpoint, "failed to derive server audio ingress endpoint");
            return false;
        }
    };
    let pcm = capture.pcm.clone();
    let sample_rate = capture.sample_rate;
    let channel_id = "embedded".to_string();
    let user_id = "embedded-user".to_string();

    let auth = match ingress_auth(device_id, keypair_path) {
        Ok(auth) => auth,
        Err(error) => {
            warn!(error = %error, "failed to build server ingress auth token");
            return false;
        }
    };

    if let Err(error) =
        upload_pcm16_to_server(endpoint, pcm, sample_rate, channel_id, user_id, auth).await
    {
        warn!(error = %error, "failed to upload captured speech to server");
        return false;
    }
    true
}

pub(super) async fn interrupt_server_active_turn(
    binding: Arc<BindingManager>,
    channel_id: String,
    user_id: String,
    reason: String,
    timestamp_us: u64,
    device_id: String,
    keypair_path: std::path::PathBuf,
) {
    let status = binding.status().await;
    let Some(server_endpoint) = status.server_endpoint.as_deref() else {
        debug!("cannot interrupt server turn before device is bound");
        return;
    };
    let endpoint = match derive_audio_ingress_endpoint(server_endpoint) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            warn!(error = %error, server_endpoint, "failed to derive server audio ingress endpoint for interrupt");
            return;
        }
    };

    let auth = match ingress_auth(&device_id, &keypair_path) {
        Ok(auth) => auth,
        Err(error) => {
            warn!(error = %error, "failed to build server interrupt auth token");
            return;
        }
    };

    if let Err(error) =
        interrupt_active_turn(endpoint, channel_id, user_id, reason, timestamp_us, auth).await
    {
        warn!(error = %error, "failed to interrupt active server voice turn");
        debug!(error = ?error, "server turn interrupt failure details");
    }
}

pub fn derive_audio_ingress_endpoint(server_endpoint: &str) -> Result<String> {
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
    let ingress_port = port
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("server endpoint port cannot be incremented"))?;
    Ok(format!("{scheme}://{host}:{ingress_port}"))
}

async fn upload_pcm16_to_server(
    endpoint: String,
    pcm: Vec<i16>,
    sample_rate: u32,
    channel_id: String,
    user_id: String,
    auth: IngressAuth,
) -> Result<()> {
    let mut client = ServerAudioIngressClient::connect(endpoint.clone())
        .await
        .with_context(|| format!("connecting to server audio ingress at {endpoint}"))?;

    let auth_channel_id = channel_id.clone();
    let upload_stream = stream::unfold(
        (pcm, 0usize, 0u64, channel_id, user_id),
        move |(pcm, offset, sequence, channel_id, user_id)| async move {
            if offset >= pcm.len() {
                return None;
            }

            let end = offset.saturating_add(1600).min(pcm.len());
            let frame = CapturedAudioChunk {
                chunk: Some(AudioChunk {
                    opus_data: pcm16_to_le_bytes(&pcm[offset..end]),
                    sequence,
                    timestamp_us: timestamp_us(),
                    encoding: AudioEncoding::PcmS16le as i32,
                }),
                channel_id: channel_id.clone(),
                user_id: user_id.clone(),
                sample_rate,
                channels: 1,
            };
            Some((frame, (pcm, end, sequence + 1, channel_id, user_id)))
        },
    );

    let mut request = tonic::Request::new(upload_stream);
    attach_ingress_auth(request.metadata_mut(), &auth, &auth_channel_id)?;

    client
        .upload_captured_audio(request)
        .await
        .context("uploading captured speech to server")?;
    Ok(())
}

async fn interrupt_active_turn(
    endpoint: String,
    channel_id: String,
    user_id: String,
    reason: String,
    timestamp_us: u64,
    auth: IngressAuth,
) -> Result<()> {
    let mut client = ServerAudioIngressClient::connect(endpoint.clone())
        .await
        .with_context(|| format!("connecting to server audio ingress at {endpoint}"))?;

    let mut request = tonic::Request::new(InterruptActiveTurnRequest {
        channel_id,
        user_id,
        reason,
        timestamp_us,
    });
    let auth_channel_id = request.get_ref().channel_id.clone();
    attach_ingress_auth(request.metadata_mut(), &auth, &auth_channel_id)?;

    client
        .interrupt_active_turn(request)
        .await
        .context("interrupting active server voice turn")?;
    Ok(())
}

#[derive(Debug, Clone)]
pub(super) struct IngressAuth {
    pub(super) token: String,
}

pub(super) fn ingress_auth(device_id: &str, keypair_path: &Path) -> Result<IngressAuth> {
    let keypair = identity::load_device_keypair(keypair_path)?;
    let public_key = keypair.public_key_bytes();
    let subject = embedded_device_auth_subject(device_id, &public_key);
    let token = mint_auth_token(
        keypair.signing_key(),
        &subject,
        EMBEDDED_INGRESS_SCOPE,
        DEFAULT_GRPC_AUTH_TTL,
    )?;
    Ok(IngressAuth { token })
}

pub(super) fn attach_ingress_auth(
    metadata: &mut tonic::metadata::MetadataMap,
    auth: &IngressAuth,
    channel_id: &str,
) -> Result<()> {
    let authorization = format!("Bearer {}", auth.token)
        .parse()
        .context("building authorization metadata")?;
    metadata.insert("authorization", authorization);

    if !channel_id.trim().is_empty() {
        metadata.insert(
            "x-alia-channel-id",
            channel_id.parse().context("building channel id metadata")?,
        );
    }
    Ok(())
}

pub(super) fn pcm16_to_le_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}

fn timestamp_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
