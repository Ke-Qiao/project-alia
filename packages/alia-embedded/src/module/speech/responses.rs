use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tracing::{error, info};

use crate::module::audio_debug::AudioDebugRecorder;
use crate::module::context::{EventBus, ModuleEvent};
use crate::module::traits::{AudioPlaybackBackend, DirectResponseAction, DirectResponseEngine};

pub(super) const ACTION_WAKEWORD: &str = "wakeword";
pub(super) const ACTION_FALLBACK: &str = "fallback";
pub(super) const ACTION_CANCEL: &str = "cancel";
pub(super) const ACTION_LISTENING_START: &str = "listening_start";
pub(super) const ACTION_LISTENING_CONTINUE: &str = "listening_continue";

pub(super) async fn play_direct_response(
    action: DirectResponseAction,
    playback: Arc<dyn AudioPlaybackBackend>,
    event_bus: EventBus,
    audio_debug: AudioDebugRecorder,
) -> bool {
    if let Some(action_name) = action.action.as_deref() {
        info!(action = action_name, "direct response action matched");
    }

    let Some(audio_path) = action.audio_path.as_deref() else {
        error!(
            action = action.action.as_deref().unwrap_or("<unspecified>"),
            "direct response audio is not configured; continuing without playback"
        );
        return false;
    };

    match read_pcm16_wav(Path::new(audio_path)).await {
        Ok(wav) => {
            audio_debug.record_playback("direct-response", &wav.pcm, wav.sample_rate, 1);
            info!(
                audio = %audio_path,
                source_sample_rate = wav.sample_rate,
                source_channels = wav.source_channels,
                pcm_samples = wav.pcm.len(),
                "playing direct response audio"
            );
            event_bus.publish(ModuleEvent::PlaybackStarted {
                source: audio_path.to_string(),
            });
            if let Err(error) = playback.play(&wav.pcm, wav.sample_rate) {
                error!(error = %error, audio = %audio_path, "direct response playback failed");
                event_bus.publish(ModuleEvent::PlaybackFinished {
                    source: audio_path.to_string(),
                });
                false
            } else if let Some(duration) = pcm_duration(&wav.pcm, wav.sample_rate) {
                tokio::time::sleep(duration).await;
                event_bus.publish(ModuleEvent::PlaybackFinished {
                    source: audio_path.to_string(),
                });
                true
            } else {
                event_bus.publish(ModuleEvent::PlaybackFinished {
                    source: audio_path.to_string(),
                });
                true
            }
        }
        Err(error) => {
            error!(error = %error, audio = %audio_path, "failed to read direct response audio");
            false
        }
    }
}

pub(super) async fn play_direct_response_action(
    action: &str,
    direct_response: Arc<dyn DirectResponseEngine>,
    playback: Arc<dyn AudioPlaybackBackend>,
    event_bus: EventBus,
    audio_debug: AudioDebugRecorder,
) -> bool {
    let Some(response) = direct_response.match_action(action) else {
        error!(
            action,
            "direct response is not configured; continuing without playback"
        );
        return false;
    };
    play_direct_response(response, playback, event_bus, audio_debug).await
}

pub(super) async fn play_direct_response_action_or_keyword(
    action: &str,
    keyword: &str,
    direct_response: Arc<dyn DirectResponseEngine>,
    playback: Arc<dyn AudioPlaybackBackend>,
    event_bus: EventBus,
    audio_debug: AudioDebugRecorder,
) -> bool {
    let response = direct_response
        .match_action(action)
        .or_else(|| direct_response.match_keyword(keyword));
    let Some(response) = response else {
        error!(
            action,
            keyword, "direct response is not configured; continuing without playback"
        );
        return false;
    };
    play_direct_response(response, playback, event_bus, audio_debug).await
}

pub(super) fn is_cancel_direct_response(action: &DirectResponseAction) -> bool {
    action
        .action
        .as_deref()
        .is_some_and(|name| normalize_response_action(name) == ACTION_CANCEL)
}

fn normalize_response_action(input: &str) -> String {
    input
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|ch| if ch == '-' { '_' } else { ch })
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect()
}

fn pcm_duration(pcm: &[i16], sample_rate: u32) -> Option<Duration> {
    if pcm.is_empty() || sample_rate == 0 {
        return None;
    }

    Some(Duration::from_secs_f64(
        pcm.len() as f64 / sample_rate as f64,
    ))
}

#[derive(Debug)]
struct DecodedPcmWav {
    pcm: Vec<i16>,
    sample_rate: u32,
    source_channels: u16,
}

async fn read_pcm16_wav(path: &Path) -> Result<DecodedPcmWav> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || read_pcm16_wav_blocking(&path))
        .await
        .context("joining WAV reader task")?
}

fn read_pcm16_wav_blocking(path: &Path) -> Result<DecodedPcmWav> {
    let data = std::fs::read(path).with_context(|| format!("reading WAV: {}", path.display()))?;
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        bail!("{} is not a RIFF/WAVE file", path.display());
    }

    let mut offset = 12;
    let mut sample_rate = None;
    let mut channels = None;
    let mut bits_per_sample = None;
    let mut audio_format = None;
    let mut pcm_bytes = None;

    while offset + 8 <= data.len() {
        let id = &data[offset..offset + 4];
        let size = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        offset += 8;
        if offset + size > data.len() {
            bail!("malformed WAV chunk in {}", path.display());
        }

        match id {
            b"fmt " => {
                if size < 16 {
                    bail!("invalid WAV fmt chunk in {}", path.display());
                }
                audio_format = Some(u16::from_le_bytes(
                    data[offset..offset + 2].try_into().unwrap(),
                ));
                channels = Some(u16::from_le_bytes(
                    data[offset + 2..offset + 4].try_into().unwrap(),
                ));
                sample_rate = Some(u32::from_le_bytes(
                    data[offset + 4..offset + 8].try_into().unwrap(),
                ));
                bits_per_sample = Some(u16::from_le_bytes(
                    data[offset + 14..offset + 16].try_into().unwrap(),
                ));
            }
            b"data" => {
                pcm_bytes = Some(&data[offset..offset + size]);
            }
            _ => {}
        }

        offset += size + (size % 2);
    }

    if audio_format != Some(1) {
        bail!("{} must be PCM WAV", path.display());
    }
    if bits_per_sample != Some(16) {
        bail!("{} must be 16-bit PCM WAV", path.display());
    }

    let channels = channels.context("missing WAV channel count")?;
    let sample_rate = sample_rate.context("missing WAV sample rate")?;
    let pcm_bytes = pcm_bytes.context("missing WAV data chunk")?;
    let samples: Vec<i16> = pcm_bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    if channels <= 1 {
        return Ok(DecodedPcmWav {
            pcm: samples,
            sample_rate,
            source_channels: channels,
        });
    }

    let mono = samples
        .chunks_exact(channels as usize)
        .map(|frame| frame[0])
        .collect();
    Ok(DecodedPcmWav {
        pcm: mono,
        sample_rate,
        source_channels: channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pcm16_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.wav");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&16_000u32.to_le_bytes());
        bytes.extend_from_slice(&32_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&1i16.to_le_bytes());
        bytes.extend_from_slice(&(-2i16).to_le_bytes());
        std::fs::write(&path, bytes).unwrap();

        let wav = read_pcm16_wav_blocking(&path).unwrap();
        assert_eq!(wav.sample_rate, 16_000);
        assert_eq!(wav.source_channels, 1);
        assert_eq!(wav.pcm, vec![1, -2]);
    }

    #[test]
    fn computes_pcm_duration() {
        assert_eq!(
            pcm_duration(&vec![0; 16_000], 16_000),
            Some(Duration::from_secs(1))
        );
        assert_eq!(pcm_duration(&[], 16_000), None);
        assert_eq!(pcm_duration(&[0], 0), None);
    }
}
