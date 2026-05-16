use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::config::AudioConfig;

pub const AUDIO_VOLUME_STATE_KEY: &str = "audio_volume";
pub const MIN_AUDIO_VOLUME: f32 = 0.0;
pub const MAX_AUDIO_VOLUME: f32 = 8.0;

const VOLUME_SCALE: f32 = 1_000.0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct AudioVolumeConfig {
    #[serde(default = "default_audio_volume")]
    pub capture_volume: f32,
    #[serde(default = "default_audio_volume")]
    pub playback_volume: f32,
}

impl AudioVolumeConfig {
    pub fn from_audio_config(config: &AudioConfig) -> Self {
        Self {
            capture_volume: config.capture_volume,
            playback_volume: config.playback_volume,
        }
        .sanitized()
    }

    pub fn sanitized(self) -> Self {
        Self {
            capture_volume: sanitize_volume(self.capture_volume),
            playback_volume: sanitize_volume(self.playback_volume),
        }
    }
}

impl Default for AudioVolumeConfig {
    fn default() -> Self {
        Self {
            capture_volume: default_audio_volume(),
            playback_volume: default_audio_volume(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AudioVolumeState {
    capture_milli: Arc<AtomicU32>,
    playback_milli: Arc<AtomicU32>,
}

impl AudioVolumeState {
    pub fn new(config: AudioVolumeConfig) -> Self {
        let config = config.sanitized();
        Self {
            capture_milli: Arc::new(AtomicU32::new(volume_to_milli(config.capture_volume))),
            playback_milli: Arc::new(AtomicU32::new(volume_to_milli(config.playback_volume))),
        }
    }

    pub fn get(&self) -> AudioVolumeConfig {
        AudioVolumeConfig {
            capture_volume: milli_to_volume(self.capture_milli.load(Ordering::Relaxed)),
            playback_volume: milli_to_volume(self.playback_milli.load(Ordering::Relaxed)),
        }
    }

    pub fn set(&self, config: AudioVolumeConfig) -> AudioVolumeConfig {
        let config = config.sanitized();
        self.capture_milli
            .store(volume_to_milli(config.capture_volume), Ordering::Relaxed);
        self.playback_milli
            .store(volume_to_milli(config.playback_volume), Ordering::Relaxed);
        config
    }

    pub fn capture_volume(&self) -> f32 {
        milli_to_volume(self.capture_milli.load(Ordering::Relaxed))
    }

    pub fn playback_volume(&self) -> f32 {
        milli_to_volume(self.playback_milli.load(Ordering::Relaxed))
    }
}

pub fn default_audio_volume() -> f32 {
    1.0
}

pub fn sanitize_volume(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_AUDIO_VOLUME, MAX_AUDIO_VOLUME)
    } else {
        default_audio_volume()
    }
}

pub fn apply_gain_in_place(pcm: &mut [i16], gain: f32) {
    let gain = sanitize_volume(gain);
    if (gain - 1.0).abs() < f32::EPSILON {
        return;
    }

    for sample in pcm {
        let scaled = (*sample as f32 * gain).round();
        *sample = scaled.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    }
}

fn volume_to_milli(value: f32) -> u32 {
    (sanitize_volume(value) * VOLUME_SCALE).round() as u32
}

fn milli_to_volume(value: u32) -> f32 {
    sanitize_volume(value as f32 / VOLUME_SCALE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_volume_state() {
        let state = AudioVolumeState::new(AudioVolumeConfig {
            capture_volume: -1.0,
            playback_volume: 99.0,
        });

        assert_eq!(state.capture_volume(), MIN_AUDIO_VOLUME);
        assert_eq!(state.playback_volume(), MAX_AUDIO_VOLUME);
    }

    #[test]
    fn applies_saturating_gain() {
        let mut pcm = vec![1_000, 20_000, -20_000];

        apply_gain_in_place(&mut pcm, 2.0);

        assert_eq!(pcm, vec![2_000, 32_767, -32_768]);
    }
}
