use std::path::{Path, PathBuf};

use alia_embedded::config::{
    EmbeddedRuntimeSettings, normalize_config_path_for_cwd, relativize_runtime_paths,
};

#[test]
fn strips_duplicate_config_prefix_when_cwd_is_config_dir() {
    let normalized =
        normalize_config_path_for_cwd(PathBuf::from("config/embedded.toml"), Path::new("config"));

    assert_eq!(normalized, PathBuf::from("embedded.toml"));
}

#[test]
fn keeps_config_prefix_when_cwd_is_project_root() {
    let normalized = normalize_config_path_for_cwd(
        PathBuf::from("config/embedded.toml"),
        Path::new("project-alia"),
    );

    assert_eq!(normalized, PathBuf::from("config/embedded.toml"));
}

#[test]
fn saves_keypair_path_relative_to_config_file() {
    let mut settings = EmbeddedRuntimeSettings::default();
    settings.identity.keypair_path = PathBuf::from("config/device.key");

    relativize_runtime_paths(Path::new("config/embedded.toml"), &mut settings);

    assert_eq!(settings.identity.keypair_path, PathBuf::from("device.key"));
}

#[test]
fn keeps_runtime_asset_dirs_relative_to_runtime_root() {
    let mut settings = EmbeddedRuntimeSettings::default();
    settings.direct_response_defaults.audio_dir = PathBuf::from("run/direct-responses");

    relativize_runtime_paths(Path::new("config/embedded.toml"), &mut settings);

    assert_eq!(
        settings.direct_response_defaults.audio_dir,
        PathBuf::from("run/direct-responses")
    );
}

#[test]
fn has_post_wake_response_delay_default() {
    let settings = EmbeddedRuntimeSettings::default();

    assert_eq!(settings.edge_stt.post_wake_response_delay_ms, 700);
}

#[test]
fn has_post_wake_min_capture_default() {
    let settings = EmbeddedRuntimeSettings::default();

    assert_eq!(settings.edge_stt.post_wake_min_capture_ms, 1_800);
}

#[test]
fn has_longer_post_wake_capture_limits() {
    let settings = EmbeddedRuntimeSettings::default();

    assert_eq!(settings.edge_stt.max_duration_ms, 15_000);
    assert_eq!(settings.vad.max_speech_duration_ms, 15_000);
}

#[test]
fn has_vad_model_path_default() {
    let settings = EmbeddedRuntimeSettings::default();

    assert_eq!(
        settings.vad.model_path,
        PathBuf::from("models/silero-vad/silero_vad.onnx")
    );
}

#[test]
fn has_interrupt_defaults() {
    let settings = EmbeddedRuntimeSettings::default();

    assert!(settings.interrupt.interrupt_on_wakeword_during_playback);
    assert!(settings.interrupt.interrupt_skip_direct_response);
    assert_eq!(settings.interrupt.interrupt_debounce_ms, 300);
}

#[test]
fn has_thinking_defaults() {
    let settings = EmbeddedRuntimeSettings::default();

    assert!(settings.thinking.enabled);
    assert_eq!(settings.thinking.action, "thinking");
    assert_eq!(settings.thinking.initial_delay_ms, 1_200);
    assert_eq!(settings.thinking.repeat_interval_ms, 3_000);
}

#[test]
fn has_continuous_dialogue_defaults() {
    let settings = EmbeddedRuntimeSettings::default();

    assert!(settings.continuous_dialogue.enabled);
    assert_eq!(settings.continuous_dialogue.followup_timeout_ms, 10_000);
    assert_eq!(settings.continuous_dialogue.max_followup_turns, 6);
}

#[test]
fn has_audio_debug_defaults() {
    let settings = EmbeddedRuntimeSettings::default();

    assert!(settings.audio.alsa_playback_follow_source_rate);
    assert!(!settings.audio.alsa_playback_lock_to_capture_rate);
    assert!(!settings.audio_debug.enabled);
    assert_eq!(settings.audio_debug.dir, PathBuf::from("run/audio-debug"));
    assert!(settings.audio_debug.record_capture_stream);
    assert!(settings.audio_debug.record_wake_capture);
    assert!(settings.audio_debug.record_playback);
    assert_eq!(settings.audio_debug.capture_segment_ms, 10_000);
}

#[test]
fn has_hmmd_debug_default() {
    let settings = EmbeddedRuntimeSettings::default();

    assert!(!settings.hmmd_mmwave.debug);
}
