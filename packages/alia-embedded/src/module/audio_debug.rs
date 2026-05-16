use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::config::AudioDebugConfig;

use super::context::AudioFrame;

#[derive(Clone)]
pub struct AudioDebugRecorder {
    config: Arc<AudioDebugConfig>,
    capture_state: Arc<Mutex<CaptureSegmentState>>,
}

#[derive(Debug, Default)]
struct CaptureSegmentState {
    sample_rate: u32,
    started_us: u64,
    pcm: Vec<i16>,
}

impl AudioDebugRecorder {
    pub fn new(config: AudioDebugConfig) -> Self {
        Self {
            config: Arc::new(config),
            capture_state: Arc::new(Mutex::new(CaptureSegmentState::default())),
        }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn record_playback_enabled(&self) -> bool {
        self.config.enabled && self.config.record_playback
    }

    pub async fn record_capture_frame(&self, frame: &AudioFrame) {
        if !self.config.enabled || !self.config.record_capture_stream || frame.pcm.is_empty() {
            return;
        }

        let segment = {
            let mut state = self.capture_state.lock().await;
            if state.sample_rate != 0 && state.sample_rate != frame.sample_rate {
                let segment = state.take();
                state.sample_rate = frame.sample_rate;
                state.started_us = frame.timestamp_us;
                state.pcm.extend_from_slice(&frame.pcm);
                segment
            } else {
                if state.sample_rate == 0 {
                    state.sample_rate = frame.sample_rate;
                    state.started_us = frame.timestamp_us;
                }
                state.pcm.extend_from_slice(&frame.pcm);
                None
            }
            .or_else(|| {
                let segment_samples =
                    samples_for_ms(state.sample_rate, 1, self.config.capture_segment_ms.max(1));
                if state.pcm.len() >= segment_samples {
                    state.take()
                } else {
                    None
                }
            })
        };

        if let Some(segment) = segment {
            self.spawn_write_wav(
                "mic-stream",
                segment.pcm,
                segment.sample_rate,
                1,
                Some(segment.started_us),
            );
        }
    }

    pub fn record_wake_capture(&self, pcm: &[i16], sample_rate: u32, keyword: &str) {
        if !self.config.enabled || !self.config.record_wake_capture || pcm.is_empty() {
            return;
        }
        self.spawn_write_wav(
            &format!("wake-capture-{keyword}"),
            pcm.to_vec(),
            sample_rate,
            1,
            None,
        );
    }

    pub fn record_playback(&self, label: &str, pcm: &[i16], sample_rate: u32, channels: u16) {
        if !self.record_playback_enabled() || pcm.is_empty() {
            return;
        }
        self.spawn_write_wav(label, pcm.to_vec(), sample_rate, channels.max(1), None);
    }

    fn spawn_write_wav(
        &self,
        label: &str,
        pcm: Vec<i16>,
        sample_rate: u32,
        channels: u16,
        timestamp_us: Option<u64>,
    ) {
        if sample_rate == 0 || pcm.is_empty() {
            return;
        }

        let dir = self.config.dir.clone();
        let path = dir.join(debug_wav_name(label, sample_rate, channels, timestamp_us));
        tokio::task::spawn_blocking(move || {
            if let Err(error) = write_pcm16_wav(&path, &pcm, sample_rate, channels) {
                warn!(error = %error, path = %path.display(), "failed to write audio debug WAV");
            } else {
                debug!(path = %path.display(), "wrote audio debug WAV");
            }
        });
    }
}

impl CaptureSegmentState {
    fn take(&mut self) -> Option<CaptureSegment> {
        if self.sample_rate == 0 || self.pcm.is_empty() {
            return None;
        }
        let segment = CaptureSegment {
            sample_rate: self.sample_rate,
            started_us: self.started_us,
            pcm: std::mem::take(&mut self.pcm),
        };
        self.sample_rate = 0;
        self.started_us = 0;
        Some(segment)
    }
}

struct CaptureSegment {
    sample_rate: u32,
    started_us: u64,
    pcm: Vec<i16>,
}

fn samples_for_ms(sample_rate: u32, channels: u16, duration_ms: u32) -> usize {
    sample_rate
        .saturating_mul(u32::from(channels.max(1)))
        .saturating_mul(duration_ms)
        .saturating_div(1_000)
        .max(1) as usize
}

fn debug_wav_name(
    label: &str,
    sample_rate: u32,
    channels: u16,
    timestamp_us: Option<u64>,
) -> String {
    let timestamp = timestamp_us
        .map(|value| value.to_string())
        .unwrap_or_else(|| Utc::now().timestamp_micros().to_string());
    format!(
        "{}-{}-{}hz-{}ch.wav",
        timestamp,
        sanitize_label(label),
        sample_rate,
        channels
    )
}

fn sanitize_label(label: &str) -> String {
    let sanitized = label
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if sanitized.is_empty() {
        "audio".to_string()
    } else {
        sanitized
    }
}

fn write_pcm16_wav(path: &Path, pcm: &[i16], sample_rate: u32, channels: u16) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating audio debug dir: {}", parent.display()))?;
    }

    let channels = channels.max(1);
    let data_bytes = (pcm.len() * 2) as u32;
    let byte_rate = sample_rate
        .saturating_mul(u32::from(channels))
        .saturating_mul(2);
    let block_align = channels.saturating_mul(2);
    let riff_size = 36_u32.saturating_add(data_bytes);
    let mut bytes = Vec::with_capacity(44 + pcm.len() * 2);

    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&riff_size.to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&byte_rate.to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in pcm {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }

    std::fs::write(path, bytes)
        .with_context(|| format!("writing audio debug WAV: {}", path.display()))
}

#[allow(dead_code)]
pub fn audio_debug_dir(config: &AudioDebugConfig) -> PathBuf {
    config.dir.clone()
}
