use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const SAMPLE_RATE_HZ: u32 = 16_000;
pub const MAX_WINDOW_MS: u32 = 30_000;
pub const MAX_WINDOW_BYTES: usize = SAMPLE_RATE_HZ as usize * 2 * MAX_WINDOW_MS as usize / 1_000;

/// A validated mono, signed 16-bit little-endian PCM window, independent of inference location.
#[derive(Clone, PartialEq, Eq)]
pub struct PcmWindow(Vec<u8>);

impl std::fmt::Debug for PcmWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PcmWindow")
            .field("bytes", &self.0.len())
            .field("duration_ms", &self.duration_ms())
            .finish()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AudioError {
    #[error("expected mono pcm_s16le at 16000 Hz")]
    UnsupportedFormat,
    #[error("PCM window must contain complete 16-bit samples")]
    InvalidLength,
    #[error("PCM window exceeds the 30 second limit")]
    WindowTooLong,
    #[error("audio sequence discontinuity: expected {expected}, received {received}")]
    Discontinuity { expected: u64, received: u64 },
    #[error("audio sequence exhausted")]
    SequenceExhausted,
}

impl PcmWindow {
    pub fn new(bytes: Vec<u8>, sample_rate_hz: u32, channels: u32) -> Result<Self, AudioError> {
        if sample_rate_hz != SAMPLE_RATE_HZ || channels != 1 {
            return Err(AudioError::UnsupportedFormat);
        }
        if bytes.is_empty() || bytes.len() % 2 != 0 {
            return Err(AudioError::InvalidLength);
        }
        if bytes.len() > MAX_WINDOW_BYTES {
            return Err(AudioError::WindowTooLong);
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn duration_ms(&self) -> u32 {
        (self.0.len() as u64 * 1_000 / (SAMPLE_RATE_HZ as u64 * 2)) as u32
    }
}

/// An utterance is invalid after a lost/reordered frame; never silently transcribe corrupted input.
#[derive(Debug, Default)]
pub struct AudioWindowBuffer {
    bytes: Vec<u8>,
    next_sequence: u64,
}

impl AudioWindowBuffer {
    pub fn push(&mut self, sequence: u64, bytes: &[u8]) -> Result<(), AudioError> {
        if sequence != self.next_sequence {
            return Err(AudioError::Discontinuity {
                expected: self.next_sequence,
                received: sequence,
            });
        }
        if bytes.is_empty() || bytes.len() % 2 != 0 {
            return Err(AudioError::InvalidLength);
        }
        if bytes.len() > MAX_WINDOW_BYTES.saturating_sub(self.bytes.len()) {
            return Err(AudioError::WindowTooLong);
        }
        let next = sequence
            .checked_add(1)
            .ok_or(AudioError::SequenceExhausted)?;
        self.bytes.extend_from_slice(bytes);
        self.next_sequence = next;
        Ok(())
    }

    pub fn snapshot(&self) -> Result<PcmWindow, AudioError> {
        PcmWindow::new(self.bytes.clone(), SAMPLE_RATE_HZ, 1)
    }

    pub fn into_window(self) -> Result<PcmWindow, AudioError> {
        PcmWindow::new(self.bytes, SAMPLE_RATE_HZ, 1)
    }

    pub fn clear_audio(&mut self) {
        self.bytes.clear();
    }

    pub fn duration_ms(&self) -> u32 {
        (self.bytes.len() as u64 * 1_000 / (SAMPLE_RATE_HZ as u64 * 2)) as u32
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    Complete,
    Incomplete,
    Backchannel,
    Wait,
}

/// Identity of one inference attempt. Epoch changes invalidate results even for the same session.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetectionId {
    pub request_id: Uuid,
    pub generation_epoch: u64,
}

impl DetectionId {
    pub fn new(generation_epoch: u64) -> Self {
        Self {
            request_id: Uuid::new_v4(),
            generation_epoch,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelIdentity {
    pub source: String,
    pub revision: String,
    pub device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnObservation {
    #[serde(flatten)]
    pub id: DetectionId,
    pub transcript: String,
    pub state: TurnState,
    pub model: ModelIdentity,
    pub inference_ms: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_format_and_size_are_validated_before_inference() {
        assert_eq!(
            PcmWindow::new(vec![0; 2], 48_000, 1),
            Err(AudioError::UnsupportedFormat)
        );
        assert_eq!(
            PcmWindow::new(vec![0; 2], 16_000, 2),
            Err(AudioError::UnsupportedFormat)
        );
        assert_eq!(
            PcmWindow::new(vec![0], 16_000, 1),
            Err(AudioError::InvalidLength)
        );
        assert_eq!(
            PcmWindow::new(vec![], 16_000, 1),
            Err(AudioError::InvalidLength)
        );
        assert_eq!(
            PcmWindow::new(vec![0; MAX_WINDOW_BYTES + 2], 16_000, 1),
            Err(AudioError::WindowTooLong)
        );
        assert_eq!(
            PcmWindow::new(vec![0; 32_000], 16_000, 1)
                .unwrap()
                .duration_ms(),
            1_000
        );
    }

    #[test]
    fn rejected_frames_do_not_mutate_buffer_or_advance_sequence() {
        let mut buffer = AudioWindowBuffer::default();
        buffer.push(0, &[1, 2]).unwrap();
        assert_eq!(
            buffer.push(2, &[3, 4]),
            Err(AudioError::Discontinuity {
                expected: 1,
                received: 2
            })
        );
        assert_eq!(buffer.push(1, &[0]), Err(AudioError::InvalidLength));
        buffer.push(1, &[3, 4]).unwrap();
        assert_eq!(buffer.into_window().unwrap().as_bytes(), &[1, 2, 3, 4]);
    }

    #[test]
    fn utterance_clear_keeps_stream_sequence_and_drops_prior_speech() {
        let mut buffer = AudioWindowBuffer::default();
        buffer.push(0, &[1, 2]).unwrap();
        buffer.clear_audio();
        buffer.push(1, &[3, 4]).unwrap();
        assert_eq!(buffer.into_window().unwrap().as_bytes(), &[3, 4]);
    }
}
