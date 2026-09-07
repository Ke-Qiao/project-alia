//! Bounded model windows assembled from a continuous device sample clock.

use crate::voice::{MAX_WINDOW_BYTES, PcmWindow, SAMPLE_RATE_HZ};

const PRE_ROLL_BYTES: usize = 9_600; // 300 ms.
const ENDPOINT_SAMPLES: u64 = 11_200; // 700 ms, an evaluation cadence, not turn completion.
pub const MAX_FRAME_SAMPLES: usize = 1_600;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CaptureError {
    #[error("capture sequence or sample clock is discontinuous")]
    Discontinuity,
    #[error("capture frame must contain 1..1600 complete PCM16 samples")]
    InvalidFrame,
    #[error("utterance exceeds the bounded 30 second model window")]
    WindowTooLong,
    #[error("capture clock exhausted")]
    ClockExhausted,
}

#[derive(Debug, Default)]
pub struct CaptureWindow {
    next_sequence: u64,
    next_sample: u64,
    revision: u64,
    silence_samples: u64,
    active: bool,
    audio: Vec<u8>,
    pre_roll: Vec<u8>,
    evaluated_revision: Option<u64>,
}

impl CaptureWindow {
    pub fn push(
        &mut self,
        sequence: u64,
        sample_offset: u64,
        pcm: &[u8],
        speech: bool,
    ) -> Result<(), CaptureError> {
        if sequence != self.next_sequence || sample_offset != self.next_sample {
            return Err(CaptureError::Discontinuity);
        }
        if pcm.is_empty() || pcm.len() % 2 != 0 || pcm.len() > MAX_FRAME_SAMPLES * 2 {
            return Err(CaptureError::InvalidFrame);
        }
        let next_sequence = sequence
            .checked_add(1)
            .ok_or(CaptureError::ClockExhausted)?;
        let next_sample = sample_offset
            .checked_add(pcm.len() as u64 / 2)
            .ok_or(CaptureError::ClockExhausted)?;
        let revision = self
            .revision
            .checked_add(u64::from(speech))
            .ok_or(CaptureError::ClockExhausted)?;
        let append = self.active && (speech || self.silence_samples < ENDPOINT_SAMPLES);
        let required = if !self.active && speech {
            self.pre_roll.len() + pcm.len()
        } else if append {
            self.audio.len() + pcm.len()
        } else {
            0
        };
        if required > MAX_WINDOW_BYTES {
            return Err(CaptureError::WindowTooLong);
        }

        self.next_sequence = next_sequence;
        self.next_sample = next_sample;
        self.revision = revision;
        if speech && !self.active {
            self.active = true;
            self.audio = std::mem::take(&mut self.pre_roll);
            self.audio.extend_from_slice(pcm);
        } else if append {
            self.audio.extend_from_slice(pcm);
        } else if !self.active {
            self.pre_roll.extend_from_slice(pcm);
            let excess = self.pre_roll.len().saturating_sub(PRE_ROLL_BYTES);
            self.pre_roll.drain(..excess);
        }
        self.silence_samples = if speech {
            0
        } else {
            self.silence_samples.saturating_add(pcm.len() as u64 / 2)
        };
        Ok(())
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn through_sequence(&self) -> u64 {
        self.next_sequence.saturating_sub(1)
    }

    /// At most one snapshot per speech revision. Incoming silence is still
    /// consumed and clock-checked while inference runs, without growing memory.
    pub fn take_ready(&mut self) -> Option<PcmWindow> {
        if !self.active
            || self.silence_samples < ENDPOINT_SAMPLES
            || self.evaluated_revision == Some(self.revision)
        {
            return None;
        }
        let window = PcmWindow::new(self.audio.clone(), SAMPLE_RATE_HZ, 1).ok()?;
        self.evaluated_revision = Some(self.revision);
        Some(window)
    }

    pub fn finish_utterance(&mut self) {
        self.active = false;
        self.audio.clear();
        self.pre_roll.clear();
        self.silence_samples = 0;
        self.evaluated_revision = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(window: &mut CaptureWindow, speech: bool) {
        window
            .push(window.next_sequence, window.next_sample, &[0; 3200], speech)
            .unwrap();
    }

    #[test]
    fn continuous_silence_is_bounded_and_cannot_trigger_inference() {
        let mut window = CaptureWindow::default();
        for _ in 0..10_000 {
            frame(&mut window, false);
        }
        assert_eq!(window.pre_roll.len(), PRE_ROLL_BYTES);
        assert!(window.take_ready().is_none());
        frame(&mut window, true);
        for _ in 0..7 {
            frame(&mut window, false);
        }
        assert_eq!(window.take_ready().unwrap().duration_ms(), 1_100);
        for _ in 0..10_000 {
            frame(&mut window, false);
        }
        assert!(window.take_ready().is_none());
        assert_eq!(window.audio.len(), 35_200);
    }

    #[test]
    fn continued_speech_extends_the_same_window_and_changes_revision() {
        let mut window = CaptureWindow::default();
        frame(&mut window, true);
        for _ in 0..7 {
            frame(&mut window, false);
        }
        let first = window.take_ready().unwrap();
        let revision = window.revision();
        frame(&mut window, true);
        for _ in 0..7 {
            frame(&mut window, false);
        }
        assert!(window.take_ready().unwrap().duration_ms() > first.duration_ms());
        assert!(window.revision() > revision);
        window.finish_utterance();
        frame(&mut window, true);
        assert_eq!(window.audio.len(), 3200);
    }

    #[test]
    fn gaps_overlong_input_and_bad_frame_sizes_are_rejected() {
        let mut window = CaptureWindow::default();
        assert_eq!(
            window.push(1, 0, &[0; 2], true),
            Err(CaptureError::Discontinuity)
        );
        assert_eq!(
            window.push(0, 0, &[0; 3], true),
            Err(CaptureError::InvalidFrame)
        );
        assert_eq!(
            window.push(0, 1, &[0; 2], true),
            Err(CaptureError::Discontinuity)
        );
        for _ in 0..300 {
            frame(&mut window, true);
        }
        assert_eq!(
            window.push(300, 480_000, &[0; 2], true),
            Err(CaptureError::WindowTooLong)
        );
        assert_eq!(window.audio.len(), MAX_WINDOW_BYTES);
    }
}
