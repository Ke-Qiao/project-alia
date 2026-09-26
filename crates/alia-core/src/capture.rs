//! Bounded model windows assembled from a continuous device sample clock.

use std::collections::VecDeque;
use std::time::SystemTime;

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
    pre_roll_received: VecDeque<(usize, SystemTime)>,
    source_received_at: Option<SystemTime>,
    evaluated_revision: Option<u64>,
    last_received_at: Option<SystemTime>,
}

impl CaptureWindow {
    /// `received_at` is supplied by the receiving host when it accepts the
    /// frame; a device-provided capture timestamp is not a trusted source time.
    pub fn push(
        &mut self,
        sequence: u64,
        sample_offset: u64,
        pcm: &[u8],
        speech: bool,
        received_at: SystemTime,
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
        self.last_received_at = Some(received_at);
        if speech && !self.active {
            self.active = true;
            self.source_received_at = Some(
                self.pre_roll_received
                    .front()
                    .map_or(received_at, |(_, received_at)| *received_at),
            );
            self.audio = std::mem::take(&mut self.pre_roll);
            self.pre_roll_received.clear();
            self.audio.extend_from_slice(pcm);
        } else if append {
            self.audio.extend_from_slice(pcm);
        } else if !self.active {
            self.pre_roll.extend_from_slice(pcm);
            self.pre_roll_received.push_back((pcm.len(), received_at));
            let excess = self.pre_roll.len().saturating_sub(PRE_ROLL_BYTES);
            self.pre_roll.drain(..excess);
            let mut remaining = excess;
            while remaining > 0 {
                let (bytes, _) = self
                    .pre_roll_received
                    .front_mut()
                    .expect("retained PCM frame");
                let removed = remaining.min(*bytes);
                *bytes -= removed;
                remaining -= removed;
                if *bytes == 0 {
                    self.pre_roll_received.pop_front();
                }
            }
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

    /// A pending utterance remains occupied even after silence or an Incomplete
    /// model result. Only its semantic owner may call finish_utterance.
    pub fn is_idle(&self) -> bool {
        !self.active
    }

    /// Host receive time, absent until the first continuous frame is accepted.
    pub fn last_received_at(&self) -> Option<SystemTime> {
        self.last_received_at
    }

    /// Trusted receiver time of the first frame retained in this utterance.
    /// Partial pre-roll trimming retains that frame's original receiver time;
    /// model snapshots, retries and continued speech never refresh it.
    pub fn source_received_at(&self) -> Option<SystemTime> {
        self.source_received_at
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
        self.pre_roll_received.clear();
        self.source_received_at = None;
        self.silence_samples = 0;
        self.evaluated_revision = None;
    }

    /// Admission was rejected before model execution; a bounded scheduler may
    /// retry the latest window without manufacturing another speech revision.
    pub fn retry_after_busy(&mut self) {
        self.evaluated_revision = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(window: &mut CaptureWindow, speech: bool) {
        window
            .push(
                window.next_sequence,
                window.next_sample,
                &[0; 3200],
                speech,
                SystemTime::UNIX_EPOCH
                    + std::time::Duration::from_millis(window.next_sequence * 100),
            )
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
    fn idle_snapshot_requires_semantic_completion_and_preserves_frame_time() {
        let mut window = CaptureWindow::default();
        assert!(window.is_idle());
        assert_eq!(window.last_received_at(), None);
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10);
        window.push(0, 0, &[0; 3200], true, at).unwrap();
        for n in 1..=7 {
            window.push(n, n * 1600, &[0; 3200], false, at).unwrap();
        }
        assert!(window.take_ready().is_some());
        assert!(!window.is_idle());
        window.finish_utterance();
        assert!(window.is_idle());
        assert_eq!(window.last_received_at(), Some(at));
        assert!(
            window
                .push(100, 0, &[0; 2], false, SystemTime::now())
                .is_err()
        );
        assert_eq!(window.last_received_at(), Some(at));
    }

    #[test]
    fn gaps_overlong_input_and_bad_frame_sizes_are_rejected() {
        let mut window = CaptureWindow::default();
        assert_eq!(
            window.push(1, 0, &[0; 2], true, SystemTime::UNIX_EPOCH),
            Err(CaptureError::Discontinuity)
        );
        assert_eq!(
            window.push(0, 0, &[0; 3], true, SystemTime::UNIX_EPOCH),
            Err(CaptureError::InvalidFrame)
        );
        assert_eq!(
            window.push(0, 1, &[0; 2], true, SystemTime::UNIX_EPOCH),
            Err(CaptureError::Discontinuity)
        );
        for _ in 0..300 {
            frame(&mut window, true);
        }
        assert_eq!(
            window.push(300, 480_000, &[0; 2], true, SystemTime::UNIX_EPOCH),
            Err(CaptureError::WindowTooLong)
        );
        assert_eq!(window.audio.len(), MAX_WINDOW_BYTES);
    }
    #[test]
    fn retained_frame_time_survives_partial_trim_snapshots_and_busy_retries() {
        let mut window = CaptureWindow::default();
        let at = |seconds| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        for (sequence, bytes) in [(0, 3200), (1, 3200), (2, 3200), (3, 1600)] {
            window
                .push(
                    sequence,
                    window.next_sample,
                    &vec![0; bytes],
                    false,
                    at(sequence),
                )
                .unwrap();
        }
        // Half the first frame remains in pre-roll. Its original acceptance
        // time must survive rather than being replaced by the speech frame time.
        window
            .push(4, window.next_sample, &[0; 3200], true, at(4))
            .unwrap();
        assert_eq!(window.source_received_at(), Some(at(0)));
        for _ in 0..7 {
            frame(&mut window, false);
        }
        assert!(window.take_ready().is_some());
        window.retry_after_busy();
        assert!(window.take_ready().is_some());
        frame(&mut window, true);
        for _ in 0..7 {
            frame(&mut window, false);
        }
        assert!(window.take_ready().is_some());
        assert_eq!(window.source_received_at(), Some(at(0)));
        window.finish_utterance();
        assert_eq!(window.source_received_at(), None);
        window
            .push(
                window.next_sequence,
                window.next_sample,
                &[0; 3200],
                true,
                at(100),
            )
            .unwrap();
        assert_eq!(window.source_received_at(), Some(at(100)));
    }

    #[test]
    fn discarded_pre_roll_times_are_bounded_and_do_not_date_a_later_utterance() {
        let mut window = CaptureWindow::default();
        for _ in 0..10_000 {
            frame(&mut window, false);
        }
        assert_eq!(window.pre_roll_received.len(), 3);
        assert_eq!(window.source_received_at(), None);
        frame(&mut window, true);
        assert_eq!(
            window.source_received_at(),
            Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(999_700))
        );
        assert!(window.pre_roll_received.is_empty());
        assert_eq!(
            window.push(
                window.next_sequence + 1,
                window.next_sample,
                &[0; 2],
                true,
                SystemTime::UNIX_EPOCH
            ),
            Err(CaptureError::Discontinuity)
        );
        assert_eq!(
            window.source_received_at(),
            Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(999_700))
        );
    }
}
