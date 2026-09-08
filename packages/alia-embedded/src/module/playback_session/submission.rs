//! Backend evidence for one immutable playback. Device callbacks update only
//! atomics; networking observes snapshots outside the realtime audio thread.
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use alia_core::delivery::{ObservationEnd, PlaybackIdentity, PlaybackProgress, SubmissionBackend};
use anyhow::{Result, ensure};

const LIVE: u8 = 0;
const SUBMITTED: u8 = 1;
const CANCELLED: u8 = 2;
const FAILED: u8 = 3;
const UNKNOWN: u8 = 4;
const OUTPUT_CLOSED: usize = 1 << (usize::BITS - 1);

#[derive(Debug)]
pub struct AudioSubmission {
    pub identity: PlaybackIdentity,
    pub generation: u64,
    pub source_sample_rate: u32,
    pub backend: SubmissionBackend,
    accepted: AtomicU64,
    submitted_output: AtomicU64,
    output_gate: AtomicUsize,
    receipt_sequence: Mutex<u64>,
    ended: AtomicBool,
    terminal: AtomicU8,
    reason: Mutex<String>,
}

impl AudioSubmission {
    #[cfg(test)]
    pub(crate) fn test(identity: PlaybackIdentity, generation: u64, rate: u32) -> Arc<Self> {
        Self::new(
            identity,
            generation,
            rate,
            SubmissionBackend {
                instance_id: uuid::Uuid::from_u128(500),
                boundary: alia_core::delivery::SubmissionBoundary::CpalCallbackBuffer,
                output_sample_rate_hz: rate,
                output_channels: 1,
            },
        )
        .unwrap()
    }
    pub fn new(
        identity: PlaybackIdentity,
        generation: u64,
        source_sample_rate: u32,
        backend: SubmissionBackend,
    ) -> Result<Arc<Self>> {
        ensure!(identity.is_valid(), "invalid playback identity");
        ensure!(
            (8000..=192000).contains(&source_sample_rate),
            "invalid source sample rate"
        );
        ensure!(
            !backend.instance_id.is_nil()
                && (8000..=384000).contains(&backend.output_sample_rate_hz)
                && (1..=32).contains(&backend.output_channels),
            "invalid backend descriptor"
        );
        Ok(Arc::new(Self {
            identity,
            generation,
            source_sample_rate,
            backend,
            accepted: 0.into(),
            submitted_output: 0.into(),
            output_gate: 0.into(),
            receipt_sequence: Mutex::new(0),
            ended: false.into(),
            terminal: LIVE.into(),
            reason: Mutex::new(String::new()),
        }))
    }

    pub fn accepted(&self) -> u64 {
        self.accepted.load(Ordering::Acquire)
    }
    pub fn end_accepted(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }
    pub fn is_live(&self) -> bool {
        self.terminal.load(Ordering::Acquire) == LIVE
    }

    /// Caller holds the backend's media/control admission lock.
    pub fn accept(&self, source_offset: u64, frames: u64) -> Result<bool> {
        if !self.is_live() {
            return Ok(false);
        }
        ensure!(!self.end_accepted(), "audio after playback End");
        ensure!(
            source_offset == self.accepted(),
            "backend source offset gap"
        );
        let total = source_offset
            .checked_add(frames)
            .ok_or_else(|| anyhow::anyhow!("source overflow"))?;
        ensure!(
            total <= u64::from(self.source_sample_rate) * 120,
            "playback source exceeds 120 seconds"
        );
        self.accepted.store(total, Ordering::Release);
        Ok(true)
    }

    /// Called only after an End marker enters the same FIFO as the media.
    pub fn accept_end(&self, total: u64) -> Result<bool> {
        if !self.is_live() {
            return Ok(false);
        }
        ensure!(total == self.accepted(), "backend End count mismatch");
        ensure!(
            !self.ended.swap(true, Ordering::AcqRel),
            "duplicate playback End"
        );
        self.check_finished();
        Ok(true)
    }

    /// Acquire before touching an output buffer or entering writei. Cancellation
    /// atomically closes admission; an acquired boundary must settle its actual
    /// result even if cancellation happens during the write.
    pub fn begin_output(self: &Arc<Self>) -> Option<SubmissionWrite> {
        let mut gate = self.output_gate.load(Ordering::Acquire);
        loop {
            if gate & OUTPUT_CLOSED != 0 || !self.is_live() {
                return None;
            }
            match self.output_gate.compare_exchange_weak(
                gate,
                gate + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(SubmissionWrite {
                        submission: self.clone(),
                        finished: false,
                    });
                }
                Err(current) => gate = current,
            }
        }
    }
    pub fn output_quiescent(&self) -> bool {
        self.output_gate.load(Ordering::Acquire) & !OUTPUT_CLOSED == 0
    }
    pub fn next_snapshot(&self) -> PlaybackProgress {
        // Push and a concurrent precise Fence share one evidence clock. Capture
        // values under the sequence lock so a later receipt cannot contain an
        // older water mark merely because its caller was descheduled.
        let mut sequence = self
            .receipt_sequence
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *sequence += 1;
        self.snapshot(*sequence)
    }

    /// CPAL: after writing complete interleaved frames to its callback buffer.
    /// ALSA: only after writei returns the number of accepted frames.
    pub fn submitted_output(&self, frames: u64) {
        self.submitted_output.fetch_add(frames, Ordering::AcqRel);
        self.check_finished();
    }

    fn expected_output(&self, source: u64) -> u64 {
        (u128::from(source) * u128::from(self.backend.output_sample_rate_hz))
            .div_ceil(u128::from(self.source_sample_rate)) as u64
    }

    fn check_finished(&self) {
        if self.end_accepted()
            && self.submitted_output.load(Ordering::Acquire)
                == self.expected_output(self.accepted())
        {
            let _ = self.terminal.compare_exchange(
                LIVE,
                SUBMITTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }

    /// A software retirement does not erase already confirmed submission or
    /// prove that the device's previously submitted tail became silent.
    pub fn terminate(&self, outcome: ObservationEnd) {
        let (kind, message) = match outcome {
            ObservationEnd::Cancelled { reason } => (CANCELLED, reason),
            ObservationEnd::Failed { reason } => (FAILED, reason),
            ObservationEnd::Unknown { reason } => (UNKNOWN, reason),
            _ => return,
        };
        self.output_gate.fetch_or(OUTPUT_CLOSED, Ordering::AcqRel);
        let mut reason = self.reason.lock().unwrap_or_else(|e| e.into_inner());
        if self.terminal.load(Ordering::Acquire) == LIVE {
            *reason = message;
            let _ =
                self.terminal
                    .compare_exchange(LIVE, kind, Ordering::Release, Ordering::Acquire);
        }
    }

    pub fn snapshot(&self, receipt_sequence: u64) -> PlaybackProgress {
        // Read the terminal release first: Finished must never be combined
        // with acceptance/End values from before its final media admission.
        let terminal = self.terminal.load(Ordering::Acquire);
        let terminal = if self.output_quiescent() {
            terminal
        } else {
            LIVE
        };
        let ended = self.end_accepted();
        let accepted = self.accepted();
        let submitted = ((u128::from(self.submitted_output.load(Ordering::Acquire))
            * u128::from(self.source_sample_rate))
            / u128::from(self.backend.output_sample_rate_hz)) as u64;
        let reason = || {
            self.reason
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        };
        PlaybackProgress {
            identity: self.identity,
            receipt_sequence,
            accepted_source_frames: accepted,
            submitted_source_frames: Some(submitted.min(accepted)),
            presented_source_frames: None,
            end_accepted: ended,
            backend: Some(self.backend.clone()),
            observation_end: match terminal {
                SUBMITTED => Some(ObservationEnd::SubmissionFinished),
                CANCELLED => Some(ObservationEnd::Cancelled { reason: reason() }),
                FAILED => Some(ObservationEnd::Failed { reason: reason() }),
                UNKNOWN => Some(ObservationEnd::Unknown { reason: reason() }),
                _ => None,
            },
        }
    }
}

/// No allocation is required at the callback boundary. Dropping an unsuccessful
/// write settles zero frames; successful partial writes settle only their return.
#[derive(Debug)]
pub struct SubmissionWrite {
    submission: Arc<AudioSubmission>,
    finished: bool,
}
impl SubmissionWrite {
    pub fn finish(mut self, output_frames: u64) {
        self.submission.submitted_output(output_frames);
        self.submission.output_gate.fetch_sub(1, Ordering::AcqRel);
        self.finished = true;
    }
}
impl Drop for SubmissionWrite {
    fn drop(&mut self) {
        if !self.finished {
            self.submission.output_gate.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alia_core::delivery::{SubmissionBoundary, VoiceScope};
    use uuid::Uuid;

    #[test]
    fn source_prefix_mapping_is_monotonic_across_rate_ratios_and_partial_submissions() {
        for (source_rate, output_rate) in [
            (44100, 48000),
            (48000, 16000),
            (192000, 8000),
            (8000, 48000),
        ] {
            let identity = PlaybackIdentity {
                scope: VoiceScope {
                    device_boot_id: Uuid::new_v4(),
                    session_id: Uuid::new_v4(),
                    generation_epoch: 1,
                },
                playback_id: Uuid::new_v4(),
                playback_sequence: 1,
            };
            let submission = AudioSubmission::new(
                identity,
                2,
                source_rate,
                SubmissionBackend {
                    instance_id: Uuid::new_v4(),
                    boundary: SubmissionBoundary::CpalCallbackBuffer,
                    output_sample_rate_hz: output_rate,
                    output_channels: 2,
                },
            )
            .unwrap();
            let source_frames = 1001;
            submission.accept(0, source_frames).unwrap();
            submission.accept_end(source_frames).unwrap();
            let output_frames = submission.expected_output(source_frames);
            let mut last = submission.snapshot(1);
            assert_eq!(last.submitted_source_frames, Some(0));
            for output_prefix in 1..=output_frames {
                submission.submitted_output(1);
                let current = submission.snapshot(output_prefix + 1);
                current.validate(Some(&last), source_frames).unwrap();
                assert_eq!(
                    current.submitted_source_frames,
                    Some(
                        (output_prefix * u64::from(source_rate) / u64::from(output_rate))
                            .min(source_frames)
                    )
                );
                assert_eq!(current.presented_source_frames, None);
                assert_eq!(
                    current.observation_end.is_some(),
                    output_prefix == output_frames
                );
                last = current;
            }
            assert_eq!(
                last.observation_end,
                Some(ObservationEnd::SubmissionFinished)
            );
            submission.terminate(ObservationEnd::Cancelled {
                reason: "late software stop".into(),
            });
            assert_eq!(
                submission.snapshot(output_frames + 2).observation_end,
                Some(ObservationEnd::SubmissionFinished),
                "a confirmed submission boundary remains true after a later software stop"
            );
        }
    }
}
