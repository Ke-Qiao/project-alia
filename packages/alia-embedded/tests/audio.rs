use alia_embedded::config::{AudioBackend, AudioConfig};
use alia_embedded::module::cpal_audio::{
    ResolvedAudioBackend, buffer_frames_for_ms, detect_alsa_s32_capture_shift,
    playback_buffered_ms, resolved_capture_backend, resolved_playback_backend,
};
use alia_embedded::module::volume::{
    AudioVolumeConfig, AudioVolumeState, MAX_AUDIO_VOLUME, MIN_AUDIO_VOLUME, apply_gain_in_place,
};

#[test]
fn computes_buffer_frames_for_milliseconds() {
    assert_eq!(buffer_frames_for_ms(24_000, 60), 1_440);
}

#[test]
fn computes_buffered_playback_milliseconds() {
    assert_eq!(playback_buffered_ms(9_600, 24_000, 2), 200);
}

#[test]
fn detects_right_justified_alsa_s32_capture() {
    let samples = [0x0001_0203, -0x0001_0203, 0x0002_0304, -0x0002_0304];
    assert_eq!(detect_alsa_s32_capture_shift(&samples), Some(8));
}

#[test]
fn detects_left_justified_alsa_s32_capture() {
    let samples = [0x0102_0300, -0x0102_0300, 0x0203_0400, -0x0203_0400];
    assert_eq!(detect_alsa_s32_capture_shift(&samples), Some(16));
}

#[test]
fn detects_left_justified_alsa_s32_capture_with_noisy_low_byte() {
    let samples = [0x1102_0311, -0x1102_0322, 0x2203_0433, -0x2203_0444];
    assert_eq!(detect_alsa_s32_capture_shift(&samples), Some(16));
}

#[cfg(target_os = "linux")]
#[test]
fn detects_alsa_pcm_names() {
    use alia_embedded::module::cpal_audio::alsa_device_name;

    assert_eq!(
        alsa_device_name("alsa:plughw:0,0", false),
        Some("plughw:0,0".to_string())
    );
    assert_eq!(
        alsa_device_name("default", false),
        Some("default".to_string())
    );
    assert_eq!(alsa_device_name("MAX98375A", false), None);
    assert_eq!(
        alsa_device_name("I2S-Duplex", true),
        Some("I2S-Duplex".to_string())
    );
}

#[test]
fn resolves_explicit_alsa_backend_from_device_name() {
    let config = AudioConfig {
        capture_device: "alsa:plughw:2,0".to_string(),
        playback_device: "plughw:0,1".to_string(),
        ..AudioConfig::default()
    };

    assert_eq!(
        resolved_capture_backend(&config),
        ResolvedAudioBackend::Alsa
    );
    assert_eq!(
        resolved_playback_backend(&config),
        ResolvedAudioBackend::Alsa
    );
}

#[test]
fn respects_explicit_backend_selection() {
    let config = AudioConfig {
        capture_backend: AudioBackend::Alsa,
        playback_backend: AudioBackend::Cpal,
        ..AudioConfig::default()
    };

    assert_eq!(
        resolved_capture_backend(&config),
        ResolvedAudioBackend::Alsa
    );
    assert_eq!(
        resolved_playback_backend(&config),
        ResolvedAudioBackend::Cpal
    );
}

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
