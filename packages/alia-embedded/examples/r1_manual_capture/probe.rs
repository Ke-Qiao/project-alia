use std::time::Duration;

use alia_core::audio_clock::{AudioDirection, AudioFormat, CaptureTiming};
use alia_embedded::module::context::AudioFrame;
use anyhow::{Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};

use super::{FIRMWARE, protocol};

const FORMAT: AudioFormat = AudioFormat {
    sample_rate_hz: 16000,
    channels: 1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Version,
    Baseline,
    Ack,
    SpeechBefore,
    Silence,
    SpeechAfter,
    Done,
}

impl Phase {
    #[cfg(target_os = "linux")]
    pub fn instruction(self) -> &'static str {
        match self {
            Self::Version => "Checking firmware; remain quiet.",
            Self::Baseline => "5s baseline; remain quiet, no wake word.",
            Self::Ack => "One manual_wakeup attempted; awaiting ACK, no retry.",
            Self::SpeechBefore => "Speak ordinary test sentences for 45s; NO wake word.",
            Self::Silence => "Remain silent until the next prompt; capture stays open.",
            Self::SpeechAfter => "Repeat ordinary test sentences for 45s; NO wake word.",
            Self::Done => "Observation complete; audio validity still needs human review.",
        }
    }
}

#[derive(Default, Clone)]
pub(super) struct Levels {
    samples: u64,
    nonzero: u64,
    clipped: u64,
    peak: u16,
    sum: f64,
    squares: f64,
}

impl Levels {
    fn add(&mut self, pcm: &[i16]) {
        for &sample in pcm {
            self.samples += 1;
            self.nonzero += u64::from(sample != 0);
            self.clipped += u64::from(sample == i16::MIN || sample == i16::MAX);
            self.peak = self.peak.max(sample.unsigned_abs());
            self.sum += f64::from(sample);
            self.squares += f64::from(sample).powi(2);
        }
    }

    pub fn report(&self) -> Value {
        let count = self.samples.max(1) as f64;
        let mean = self.sum / count;
        json!({
            "samples": self.samples, "audio_seconds": self.samples as f64 / 16000.0,
            "nonzero_samples": self.nonzero, "clipped_samples": self.clipped,
            "peak": self.peak, "mean": mean,
            "rms": (self.squares / count).sqrt(),
            "ac_rms": (self.squares / count - mean * mean).max(0.0).sqrt(),
        })
    }
}

pub(super) struct Probe {
    pub phase: Phase,
    pub phase_started: Duration,
    pub version: Option<String>,
    pub manual_attempts: u8,
    pub manual_ack: bool,
    pub event_frames: u64,
    pub wake_events: u64,
    pub version_id: u16,
    pub manual_id: u16,
    pub last_timing: Option<CaptureTiming>,
    pub total_samples: u64,
    pub window: Levels,
    pub last_audio: Duration,
    beam: u8,
    silence: Duration,
    levels: [Levels; 7],
    version_requery_pending: bool,
    version_requery_attempted: bool,
    native_reference_lost_blocks: u64,
    native_reference_lost_frames: u64,
}

impl Probe {
    pub fn new(beam: u8, silence: Duration, version_id: u16) -> Self {
        Self {
            phase: Phase::Version,
            phase_started: Duration::ZERO,
            version: None,
            manual_attempts: 0,
            manual_ack: false,
            event_frames: 0,
            wake_events: 0,
            version_id,
            manual_id: version_id.wrapping_add(1),
            last_timing: None,
            total_samples: 0,
            window: Levels::default(),
            last_audio: Duration::ZERO,
            beam,
            silence,
            levels: std::array::from_fn(|_| Levels::default()),
            version_requery_pending: false,
            version_requery_attempted: false,
            native_reference_lost_blocks: 0,
            native_reference_lost_frames: 0,
        }
    }

    fn enter(&mut self, phase: Phase, now: Duration) {
        self.phase = phase;
        self.phase_started = now;
    }

    pub fn receive(&mut self, frame: &protocol::Frame, now: Duration) -> Result<Option<Vec<u8>>> {
        self.event_frames += u64::from(frame.kind == 4);
        self.wake_events += u64::from(frame.wakeup().is_some());
        if let Some(ack) = frame.handshake_ack() {
            ensure!(
                self.version.is_none(),
                "board handshake after version verification; session may have restarted"
            );
            // A cold board may ignore the initial query until its handshake is acknowledged.
            self.version_requery_pending = !self.version_requery_attempted;
            return Ok(Some(ack));
        }
        let expected = match self.phase {
            Phase::Version => self.version_id,
            Phase::Ack => self.manual_id,
            _ => return Ok(None),
        };
        if frame.kind != 5 || frame.id != expected {
            return Ok(None);
        }
        ensure!(
            now.saturating_sub(self.phase_started) < Duration::from_secs(2),
            if self.phase == Phase::Version {
                "late version response; manual_wakeup was not sent"
            } else {
                "late manual_wakeup ACK; side effect may have occurred, no retry"
            }
        );
        let body: Value = serde_json::from_slice(&frame.payload)?;
        ensure!(
            body.get("code").and_then(Value::as_i64) == Some(0),
            "command ACK rejected or missing numeric code:0"
        );
        if self.phase == Phase::Version {
            self.version = frame.version_response(expected);
            ensure!(
                self.version.as_deref() == Some(FIRMWARE),
                "firmware does not match pinned current firmware; manual_wakeup was not sent"
            );
            self.enter(Phase::Baseline, now);
        } else {
            self.manual_ack = true;
            self.enter(Phase::SpeechBefore, now);
        }
        Ok(None)
    }

    pub fn tick(&mut self, now: Duration) -> Result<Option<Vec<u8>>> {
        let elapsed = now.saturating_sub(self.phase_started);
        match self.phase {
            Phase::Version => {
                ensure!(
                    elapsed < Duration::from_secs(2),
                    "version query timed out; manual_wakeup was not sent"
                );
                if self.version_requery_pending {
                    self.version_requery_pending = false;
                    self.version_requery_attempted = true;
                    return Ok(Some(protocol::version_query(self.version_id)));
                }
            }
            Phase::Ack => ensure!(
                elapsed < Duration::from_secs(2),
                "manual_wakeup ACK timed out; side effect may have occurred, no retry"
            ),
            Phase::Baseline if elapsed >= Duration::from_secs(5) => {
                ensure!(
                    self.levels[Phase::Baseline as usize].samples > 0,
                    "no baseline PCM"
                );
                ensure!(
                    self.event_frames == 0,
                    "board event before manual command; cold-start isolation is contaminated"
                );
                ensure!(
                    self.manual_attempts == 0,
                    "manual command already attempted"
                );
                self.manual_attempts = 1;
                self.enter(Phase::Ack, now);
                return Ok(Some(protocol::encode(
                    5,
                    self.manual_id,
                    &serde_json::to_vec(
                        &json!({"type":"manual_wakeup","content":{"beam":self.beam}}),
                    )?,
                )));
            }
            Phase::SpeechBefore if elapsed >= Duration::from_secs(45) => {
                self.enter(Phase::Silence, now)
            }
            Phase::Silence if elapsed >= self.silence => self.enter(Phase::SpeechAfter, now),
            Phase::SpeechAfter if elapsed >= Duration::from_secs(45) => {
                self.enter(Phase::Done, now)
            }
            _ => {}
        }
        Ok(None)
    }

    pub fn audio(&mut self, frame: &AudioFrame, now: Duration) -> Result<()> {
        let timing = frame
            .timing
            .ok_or_else(|| anyhow::anyhow!("missing capture timing"))?;
        ensure!(
            frame.sample_rate == 16000
                && !frame.pcm.is_empty()
                && timing.converted_format == FORMAT
                && timing.native.format == FORMAT
                && timing.native.stream.direction == AudioDirection::Capture
                && !timing.resampled
                && timing.converted_frames == frame.pcm.len() as u64
                && timing.native.frames == timing.converted_frames,
            "expected native 16kHz mono PCM without resampling"
        );
        if let Some(last) = self.last_timing {
            ensure!(
                timing.native.stream == last.native.stream
                    && timing.native.continuity == last.native.continuity
                    && timing.native.start_frame == last.native.start_frame + last.native.frames
                    && timing.converted_start_frame
                        == last.converted_start_frame + last.converted_frames,
                "capture discontinuity, reopened stream or lost PCM"
            );
        } else {
            ensure!(
                timing.native.start_frame == 0 && timing.converted_start_frame == 0,
                "capture prefix missing"
            );
        }
        self.last_timing = Some(timing);
        self.last_audio = now;
        self.total_samples += frame.pcm.len() as u64;
        self.levels[self.phase as usize].add(&frame.pcm);
        self.window.add(&frame.pcm);
        Ok(())
    }

    pub fn check_audio_progress(&self, now: Duration) -> Result<()> {
        ensure!(
            now.saturating_sub(self.last_audio) < Duration::from_secs(2),
            "no PCM received for 2s (different from silent PCM)"
        );
        Ok(())
    }

    pub fn reference_loss(&mut self, lost_blocks: u64, lost_frames: u64) -> Option<Value> {
        // Reference publication is best-effort; AudioFrame timing independently checks saved PCM.
        let blocks = self.native_reference_lost_blocks.max(lost_blocks);
        let frames = self.native_reference_lost_frames.max(lost_frames);
        if (blocks, frames)
            == (
                self.native_reference_lost_blocks,
                self.native_reference_lost_frames,
            )
        {
            return None;
        }
        self.native_reference_lost_blocks = blocks;
        self.native_reference_lost_frames = frames;
        Some(json!({"type":"native_reference_loss", "lost_blocks":blocks,
            "lost_frames":frames, "native_reference_complete":false,
            "scope":"auxiliary_reference_only; received_PCM_continuity_checked_separately"}))
    }

    pub fn report(&self) -> Value {
        let phases: Vec<_> = [
            Phase::Version,
            Phase::Baseline,
            Phase::Ack,
            Phase::SpeechBefore,
            Phase::Silence,
            Phase::SpeechAfter,
        ]
        .into_iter()
        .map(|phase| json!({"phase":phase,"levels":self.levels[phase as usize].report()}))
        .collect();
        json!({"phase": self.phase, "observed_firmware": self.version,
            "version_requery_attempted":self.version_requery_attempted,
            "manual_send_attempts":self.manual_attempts, "manual_ack":self.manual_ack,
            "board_event_frames":self.event_frames, "parsed_wake_events":self.wake_events,
            "manual_only_isolation": if self.event_frames == 0 { "no_board_event_observed" } else { "requires_event_review" },
            "total_samples":self.total_samples, "last_timing":self.last_timing, "phases":phases,
            "native_reference_complete":self.total_samples > 0
                && self.native_reference_lost_blocks == 0 && self.native_reference_lost_frames == 0,
            "native_reference_lost_blocks":self.native_reference_lost_blocks,
            "native_reference_lost_frames":self.native_reference_lost_frames,
            "audio_validity":"not_established_by_statistics", "human_audio_review":"not_performed",
            "cold_boot_verified_by_tool":false})
    }
}
