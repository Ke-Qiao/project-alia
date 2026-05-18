use std::time::Duration;

use alia_embedded::config::ContinuousDialogueConfig;
use alia_embedded::module::speech::capture::{
    SpeechCapture, SpeechCaptureOrigin, SpeechCompletionReason,
};
use alia_embedded::module::speech::continuous::ContinuousDialogueState;
use alia_embedded::module::speech::is_assistant_playback_source;
use alia_embedded::module::speech::responses::{pcm_duration, read_pcm16_wav_blocking};
use alia_embedded::module::speech::transport::derive_audio_ingress_endpoint;
use alia_embedded::module::traits::{VoiceActivityState, WakeWordEvent};

#[test]
fn vad_capture_waits_for_silence_until_max_duration() {
    let mut capture = test_capture(true);

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
    let mut capture = test_capture(true);

    capture.observe_vad(VoiceActivityState::Finished);

    assert_eq!(capture.completion_reason(1_000 + 1_200_000), None);
    assert_eq!(
        capture.completion_reason(1_000 + 1_800_000),
        Some(SpeechCompletionReason::VadFinished)
    );
}

#[test]
fn vad_capture_resumes_after_premature_finished_segment() {
    let mut capture = test_capture(true);

    capture.observe_vad(VoiceActivityState::Finished);
    capture.observe_vad(VoiceActivityState::Speech);

    assert_eq!(capture.completion_reason(1_000 + 2_000_000), None);
}

#[test]
fn vad_capture_uses_no_speech_timeout_before_speech_starts() {
    let capture = test_capture(true);

    assert_eq!(capture.completion_reason(1_000 + 2_400_000), None);
    assert_eq!(
        capture.completion_reason(1_000 + 2_500_000),
        Some(SpeechCompletionReason::NoSpeechTimeout)
    );
}

#[test]
fn fixed_capture_fallback_uses_post_wake_capture_window() {
    let capture = test_capture(false);

    assert_eq!(capture.completion_reason(1_000 + 2_400_000), None);
    assert_eq!(
        capture.completion_reason(1_000 + 2_500_000),
        Some(SpeechCompletionReason::FixedWindow)
    );
}

#[test]
fn continuous_dialogue_starts_followup_after_assistant_reply() {
    let mut state = ContinuousDialogueState::new(ContinuousDialogueConfig {
        enabled: true,
        followup_timeout_ms: 10_000,
        max_followup_turns: 6,
    });

    state.start_session();
    state.mark_waiting_for_assistant();
    state.schedule_followup();

    assert!(state.start_followup_if_scheduled(1_000));
    assert!(state.is_followup_listening());
    assert!(!state.expire_if_needed(1_000 + 9_999_000));
    assert!(state.expire_if_needed(1_000 + 10_000_000));
    assert!(!state.is_followup_listening());
}

#[test]
fn continuous_dialogue_counts_followup_turns() {
    let mut state = ContinuousDialogueState::new(ContinuousDialogueConfig {
        enabled: true,
        followup_timeout_ms: 10_000,
        max_followup_turns: 1,
    });

    state.start_session();
    state.schedule_followup();
    assert!(state.start_followup_if_scheduled(1_000));
    state.mark_followup_capture_started();
    state.schedule_followup();

    assert!(!state.start_followup_if_scheduled(2_000));
    assert!(!state.is_followup_listening());
}

#[test]
fn reads_pcm16_wav() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.wav");
    std::fs::write(&path, test_wav(&[1, -2], 16_000, 1)).unwrap();

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

#[test]
fn derives_audio_ingress_from_bound_server_endpoint() {
    let endpoint = derive_audio_ingress_endpoint("http://192.168.1.10:3000").unwrap();
    assert_eq!(endpoint, "http://192.168.1.10:3001");
}

#[test]
fn assistant_playback_source_is_tts_only() {
    assert!(is_assistant_playback_source("tts"));
    assert!(!is_assistant_playback_source("wakeword_interrupt"));
    assert!(!is_assistant_playback_source(
        "run/direct-responses/wakeword/hello.wav"
    ));
}

fn test_capture(vad_enabled: bool) -> SpeechCapture {
    SpeechCapture::new(
        wakeword_event(),
        SpeechCaptureOrigin::Wakeword,
        16_000,
        1_000,
        Duration::from_millis(2_500),
        Duration::from_millis(1_800),
        Duration::from_millis(5_000),
        vad_enabled,
    )
}

fn wakeword_event() -> WakeWordEvent {
    WakeWordEvent {
        keyword: "HEY_ALIA".to_string(),
        confidence: 1.0,
        timestamp_us: 1_000,
    }
}

fn test_wav(pcm: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    let pcm = pcm
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    let mut wav = Vec::new();
    let data_len = pcm.len() as u32;
    let byte_rate = sample_rate * u32::from(channels) * 2;
    let block_align = channels * 2;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&pcm);
    wav
}
