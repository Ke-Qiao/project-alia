use std::time::Duration;

use tracing::{debug, warn};

use crate::module::context::AudioFrame;
use crate::module::traits::{VoiceActivityEngine, VoiceActivityState, WakeWordEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpeechCaptureOrigin {
    Wakeword,
    Followup,
}

impl SpeechCaptureOrigin {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Wakeword => "wakeword",
            Self::Followup => "continuous_followup",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct PendingSpeechCapture {
    pub(super) event: WakeWordEvent,
    pub(super) origin: SpeechCaptureOrigin,
    pub(super) reset_vad: bool,
    pub(super) initial_vad_state: Option<VoiceActivityState>,
}

impl PendingSpeechCapture {
    pub(super) fn wakeword(event: WakeWordEvent) -> Self {
        Self {
            event,
            origin: SpeechCaptureOrigin::Wakeword,
            reset_vad: true,
            initial_vad_state: None,
        }
    }

    pub(super) fn followup(timestamp_us: u64, initial_vad_state: VoiceActivityState) -> Self {
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

pub(super) struct SpeechCapture {
    pub(super) wakeword: WakeWordEvent,
    pub(super) origin: SpeechCaptureOrigin,
    pub(super) sample_rate: u32,
    fallback_deadline_us: u64,
    min_vad_end_deadline_us: u64,
    max_deadline_us: u64,
    vad_enabled: bool,
    vad_saw_speech: bool,
    vad_finished: bool,
    skip_next_vad_accept: bool,
    pub(super) pcm: Vec<i16>,
}

impl SpeechCapture {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
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

    pub(super) fn push(&mut self, frame: &AudioFrame) {
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

    pub(super) fn vad_enabled(&self) -> bool {
        self.vad_enabled
    }

    pub(super) fn disable_vad(&mut self) {
        self.vad_enabled = false;
    }

    pub(super) fn skip_next_vad_accept(&mut self) {
        self.skip_next_vad_accept = true;
    }

    pub(super) fn take_skip_next_vad_accept(&mut self) -> bool {
        let skip = self.skip_next_vad_accept;
        self.skip_next_vad_accept = false;
        skip
    }

    pub(super) fn observe_vad(&mut self, state: VoiceActivityState) {
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

    pub(super) fn completion_reason(&self, timestamp_us: u64) -> Option<SpeechCompletionReason> {
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

    pub(super) fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        (self.pcm.len() as u64 * 1_000) / self.sample_rate as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpeechCompletionReason {
    FixedWindow,
    VadFinished,
    MaxDuration,
    NoSpeechTimeout,
}

impl SpeechCompletionReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::FixedWindow => "fixed_window",
            Self::VadFinished => "vad_finished",
            Self::MaxDuration => "max_duration",
            Self::NoSpeechTimeout => "no_speech_timeout",
        }
    }
}

pub(super) fn prepare_vad_for_capture(vad: &dyn VoiceActivityEngine) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vad_capture_waits_for_silence_until_max_duration() {
        let mut capture = SpeechCapture::new(
            wakeword_event(),
            SpeechCaptureOrigin::Wakeword,
            16_000,
            1_000,
            Duration::from_millis(2_500),
            Duration::from_millis(1_800),
            Duration::from_millis(5_000),
            true,
        );

        capture.observe_vad(VoiceActivityState::Speech);

        assert_eq!(capture.completion_reason(1_000 + 2_600_000), None);
        capture.observe_vad(VoiceActivityState::Finished);
        assert_eq!(
            capture.completion_reason(1_000 + 2_700_000),
            Some(SpeechCompletionReason::VadFinished)
        );
    }

    #[test]
    fn vad_capture_does_not_accept_early_finished_segment() {
        let mut capture = SpeechCapture::new(
            wakeword_event(),
            SpeechCaptureOrigin::Wakeword,
            16_000,
            1_000,
            Duration::from_millis(2_500),
            Duration::from_millis(1_800),
            Duration::from_millis(5_000),
            true,
        );

        capture.observe_vad(VoiceActivityState::Finished);

        assert_eq!(capture.completion_reason(1_000 + 1_200_000), None);
        assert_eq!(
            capture.completion_reason(1_000 + 1_800_000),
            Some(SpeechCompletionReason::VadFinished)
        );
    }

    #[test]
    fn vad_capture_resumes_after_premature_finished_segment() {
        let mut capture = SpeechCapture::new(
            wakeword_event(),
            SpeechCaptureOrigin::Wakeword,
            16_000,
            1_000,
            Duration::from_millis(2_500),
            Duration::from_millis(1_800),
            Duration::from_millis(5_000),
            true,
        );

        capture.observe_vad(VoiceActivityState::Finished);
        capture.observe_vad(VoiceActivityState::Speech);

        assert_eq!(capture.completion_reason(1_000 + 2_000_000), None);
    }

    #[test]
    fn vad_capture_uses_no_speech_timeout_before_speech_starts() {
        let capture = SpeechCapture::new(
            wakeword_event(),
            SpeechCaptureOrigin::Wakeword,
            16_000,
            1_000,
            Duration::from_millis(2_500),
            Duration::from_millis(1_800),
            Duration::from_millis(5_000),
            true,
        );

        assert_eq!(capture.completion_reason(1_000 + 2_400_000), None);
        assert_eq!(
            capture.completion_reason(1_000 + 2_500_000),
            Some(SpeechCompletionReason::NoSpeechTimeout)
        );
    }

    #[test]
    fn fixed_capture_fallback_uses_post_wake_capture_window() {
        let capture = SpeechCapture::new(
            wakeword_event(),
            SpeechCaptureOrigin::Wakeword,
            16_000,
            1_000,
            Duration::from_millis(2_500),
            Duration::from_millis(1_800),
            Duration::from_millis(5_000),
            false,
        );

        assert_eq!(capture.completion_reason(1_000 + 2_400_000), None);
        assert_eq!(
            capture.completion_reason(1_000 + 2_500_000),
            Some(SpeechCompletionReason::FixedWindow)
        );
    }

    fn wakeword_event() -> WakeWordEvent {
        WakeWordEvent {
            keyword: "HEY_ALIA".to_string(),
            confidence: 1.0,
            timestamp_us: 1_000,
        }
    }
}
