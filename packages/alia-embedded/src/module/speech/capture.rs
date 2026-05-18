use std::time::Duration;

use tracing::{debug, warn};

use crate::module::context::AudioFrame;
use crate::module::traits::{VoiceActivityEngine, VoiceActivityState, WakeWordEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechCaptureOrigin {
    Wakeword,
    Followup,
}

impl SpeechCaptureOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wakeword => "wakeword",
            Self::Followup => "continuous_followup",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PendingSpeechCapture {
    pub event: WakeWordEvent,
    pub origin: SpeechCaptureOrigin,
    pub reset_vad: bool,
    pub initial_vad_state: Option<VoiceActivityState>,
}

impl PendingSpeechCapture {
    pub fn wakeword(event: WakeWordEvent) -> Self {
        Self {
            event,
            origin: SpeechCaptureOrigin::Wakeword,
            reset_vad: true,
            initial_vad_state: None,
        }
    }

    pub fn followup(timestamp_us: u64, initial_vad_state: VoiceActivityState) -> Self {
        Self {
            event: WakeWordEvent {
                keyword: "continuous_followup".to_string(),
                confidence: 1.0,
                timestamp_us,
            },
            origin: SpeechCaptureOrigin::Followup,
            reset_vad: false,
            initial_vad_state: Some(initial_vad_state),
        }
    }
}

pub struct SpeechCapture {
    pub wakeword: WakeWordEvent,
    pub origin: SpeechCaptureOrigin,
    pub sample_rate: u32,
    fallback_deadline_us: u64,
    min_vad_end_deadline_us: u64,
    max_deadline_us: u64,
    vad_enabled: bool,
    vad_saw_speech: bool,
    vad_finished: bool,
    skip_next_vad_accept: bool,
    pub pcm: Vec<i16>,
}

impl SpeechCapture {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        wakeword: WakeWordEvent,
        origin: SpeechCaptureOrigin,
        sample_rate: u32,
        started_at_us: u64,
        fallback_duration: Duration,
        min_vad_end_duration: Duration,
        max_duration: Duration,
        vad_enabled: bool,
    ) -> Self {
        let fallback_deadline_us =
            started_at_us.saturating_add(fallback_duration.as_micros() as u64);
        let min_vad_end_deadline_us =
            started_at_us.saturating_add(min_vad_end_duration.as_micros() as u64);
        let max_deadline_us = started_at_us.saturating_add(max_duration.as_micros() as u64);
        Self {
            wakeword,
            origin,
            sample_rate,
            fallback_deadline_us,
            min_vad_end_deadline_us,
            max_deadline_us,
            vad_enabled,
            vad_saw_speech: false,
            vad_finished: false,
            skip_next_vad_accept: false,
            pcm: Vec::new(),
        }
    }

    pub fn push(&mut self, frame: &AudioFrame) {
        if frame.sample_rate != self.sample_rate {
            warn!(
                expected = self.sample_rate,
                actual = frame.sample_rate,
                "dropping frame with changed sample rate during speech capture"
            );
            return;
        }
        self.pcm.extend_from_slice(&frame.pcm);
    }

    pub fn vad_enabled(&self) -> bool {
        self.vad_enabled
    }

    pub fn disable_vad(&mut self) {
        self.vad_enabled = false;
    }

    pub fn skip_next_vad_accept(&mut self) {
        self.skip_next_vad_accept = true;
    }

    pub fn take_skip_next_vad_accept(&mut self) -> bool {
        let skip = self.skip_next_vad_accept;
        self.skip_next_vad_accept = false;
        skip
    }

    pub fn observe_vad(&mut self, state: VoiceActivityState) {
        match state {
            VoiceActivityState::Unavailable => self.disable_vad(),
            VoiceActivityState::Silence => {}
            VoiceActivityState::Speech => {
                self.vad_saw_speech = true;
                self.vad_finished = false;
            }
            VoiceActivityState::Finished => {
                self.vad_saw_speech = true;
                self.vad_finished = true;
            }
        }
    }

    pub fn completion_reason(&self, timestamp_us: u64) -> Option<SpeechCompletionReason> {
        if !self.vad_enabled {
            return (timestamp_us >= self.fallback_deadline_us)
                .then_some(SpeechCompletionReason::FixedWindow);
        }

        if self.vad_finished && timestamp_us >= self.min_vad_end_deadline_us {
            return Some(SpeechCompletionReason::VadFinished);
        }
        if timestamp_us >= self.max_deadline_us {
            return Some(SpeechCompletionReason::MaxDuration);
        }
        if !self.vad_saw_speech && timestamp_us >= self.fallback_deadline_us {
            return Some(SpeechCompletionReason::NoSpeechTimeout);
        }
        None
    }

    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        (self.pcm.len() as u64 * 1_000) / self.sample_rate as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechCompletionReason {
    FixedWindow,
    VadFinished,
    MaxDuration,
    NoSpeechTimeout,
}

impl SpeechCompletionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FixedWindow => "fixed_window",
            Self::VadFinished => "vad_finished",
            Self::MaxDuration => "max_duration",
            Self::NoSpeechTimeout => "no_speech_timeout",
        }
    }
}

pub fn prepare_vad_for_capture(vad: &dyn VoiceActivityEngine) -> bool {
    if !vad.is_available() {
        return false;
    }

    match vad.reset() {
        Ok(()) => true,
        Err(error) => {
            warn!(error = %error, "failed to reset VAD before speech capture; using fixed capture window");
            debug!(error = ?error, "VAD reset failure details");
            false
        }
    }
}
