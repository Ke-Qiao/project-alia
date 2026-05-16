use std::sync::Arc;

use alia_proto::audio::server_audio_ingress_client::ServerAudioIngressClient;
use alia_proto::audio::{
    AudioChunk, AudioEncoding, CapturedAudioChunk, InterruptActiveTurnRequest,
};
use anyhow::{Context, Result};
use futures::stream;
use tracing::{debug, warn};

use crate::binding::BindingManager;

use super::capture::SpeechCapture;

pub(super) async fn upload_to_server(capture: &SpeechCapture, binding: &BindingManager) -> bool {
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

    if let Err(error) =
        upload_pcm16_to_server(endpoint, pcm, sample_rate, channel_id, user_id).await
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

    if let Err(error) =
        interrupt_active_turn(endpoint, channel_id, user_id, reason, timestamp_us).await
    {
        warn!(error = %error, "failed to interrupt active server voice turn");
        debug!(error = ?error, "server turn interrupt failure details");
    }
}

fn derive_audio_ingress_endpoint(server_endpoint: &str) -> Result<String> {
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
) -> Result<()> {
    let mut client = ServerAudioIngressClient::connect(endpoint.clone())
        .await
        .with_context(|| format!("connecting to server audio ingress at {endpoint}"))?;

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

    client
        .upload_captured_audio(upload_stream)
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
) -> Result<()> {
    let mut client = ServerAudioIngressClient::connect(endpoint.clone())
        .await
        .with_context(|| format!("connecting to server audio ingress at {endpoint}"))?;

    client
        .interrupt_active_turn(InterruptActiveTurnRequest {
            channel_id,
            user_id,
            reason,
            timestamp_us,
        })
        .await
        .context("interrupting active server voice turn")?;
    Ok(())
}

fn pcm16_to_le_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}

fn timestamp_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_audio_ingress_from_bound_server_endpoint() {
        let endpoint = derive_audio_ingress_endpoint("http://192.168.1.10:3000").unwrap();
        assert_eq!(endpoint, "http://192.168.1.10:3001");
    }
}
