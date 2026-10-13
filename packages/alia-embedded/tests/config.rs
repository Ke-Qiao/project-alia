use std::path::{Path, PathBuf};

use alia_embedded::config::{
    EmbeddedRuntimeSettings, normalize_config_path_for_cwd, relativize_runtime_paths,
};

#[test]
fn obsolete_source_wav_recorder_configuration_is_rejected() {
    for enabled in [false, true] {
        let text = format!("[audio_debug]\nenabled = {enabled}\n");
        assert!(
            toml::from_str::<EmbeddedRuntimeSettings>(&text).is_err(),
            "source WAV recording is not bounded native dual-stream evidence"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embedded.toml");
        std::fs::write(&path, &text).unwrap();
        assert!(alia_embedded::config::load_config(&path).is_err());
    }
}

#[tokio::test]
async fn obsolete_source_wav_update_preserves_memory_and_disk() {
    use alia_embedded::config::{load_config, save_runtime_config};
    use alia_embedded::settings::EmbeddedSettingsManager;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("embedded.toml");
    let config = load_config(&path).unwrap();
    save_runtime_config(&config).unwrap();
    let manager = EmbeddedSettingsManager::new(&config);
    let before_memory = manager.snapshot_json(false).await.unwrap();
    let before_disk = std::fs::read(&path).unwrap();
    let update = manager
        .update_from_json(r#"{"audio_debug":{"enabled":true}}"#)
        .await;
    assert!(update.is_err(), "removed recorder must not be re-enabled");
    assert_eq!(manager.snapshot_json(false).await.unwrap(), before_memory);
    assert_eq!(std::fs::read(&path).unwrap(), before_disk);
}

#[test]
fn removed_voice_and_mixer_selectors_are_rejected_instead_of_ignored() {
    for old in [
        "[speech]\nmode = 'utterance'",
        "[speech]\nmode = 'server_turn'",
        "[modules]\nmixer = 'noop'",
        "[modules]\nedge_stt = 'noop'",
        "[continuous_dialogue]\nenabled = true",
        "[edge_stt]\nmax_duration_ms = 15000",
    ] {
        assert!(
            toml::from_str::<EmbeddedRuntimeSettings>(old).is_err(),
            "accepted removed selector: {old}"
        );
    }
    let settings = EmbeddedRuntimeSettings::default();
    assert_eq!(settings.speech.channel_id, "embedded");
    assert!(settings.speech.validate(&settings.audio).is_ok());
    let mut invalid_audio = settings.audio;
    invalid_audio.capture_sample_rate = 48000;
    assert!(settings.speech.validate(&invalid_audio).is_err());
}

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
fn has_vad_model_path_default() {
    let settings = EmbeddedRuntimeSettings::default();

    assert_eq!(
        settings.vad.model_path,
        PathBuf::from("models/silero-vad/silero_vad.onnx")
    );
}

#[test]
fn defaults_do_not_enable_recording_or_assume_shared_audio_clock() {
    let settings = EmbeddedRuntimeSettings::default();
    assert!(!settings.audio.alsa_playback_lock_to_capture_rate);
    assert!(
        serde_json::to_value(&settings)
            .unwrap()
            .get("audio_debug")
            .is_none()
    );
}

#[test]
fn has_hmmd_debug_default() {
    let settings = EmbeddedRuntimeSettings::default();

    assert!(!settings.hmmd_mmwave.debug);
}

#[test]
fn has_motion_protection_defaults() {
    let settings = EmbeddedRuntimeSettings::default();

    assert_eq!(settings.motion.servo_backend, "serial");
    assert_eq!(settings.motion.serial_path, PathBuf::from("/dev/ttyACM0"));
    assert_eq!(settings.motion.serial_baud_rate, 115_200);
    assert_eq!(settings.motion.serial_startup_delay_ms, 2_000);
    assert!(!settings.motion.hardware_enabled);
    assert!(!settings.motion.calibration_confirmed);
    assert_eq!(settings.motion.max_servo_speed_deg_per_sec, 60.0);
}

#[test]
fn removed_motion_reset_and_soft_start_options_are_rejected_instead_of_ignored() {
    use alia_embedded::config::{load_config, save_runtime_config};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("embedded.toml");
    for old in [
        "reset_on_stop = true",
        "reset_on_stop = false",
        "reset_duration_ms = 1200",
        "soft_start_enabled = true",
        "soft_start_enabled = false",
        "soft_start_duration_ms = 1500",
    ] {
        let content = format!("[motion]\n{old}\n");
        assert!(
            toml::from_str::<EmbeddedRuntimeSettings>(&content).is_err(),
            "accepted removed motion option: {old}"
        );
        std::fs::write(&path, content).unwrap();
        assert!(
            load_config(&path).is_err(),
            "load accepted removed option: {old}"
        );
    }
    std::fs::write(&path, "[motion]\ncalibration_confirmed = true\n").unwrap();
    let config = load_config(&path).unwrap();
    assert!(config.motion.calibration_confirmed);
    save_runtime_config(&config).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    for removed in [
        "reset_on_stop",
        "reset_duration_ms",
        "soft_start_enabled",
        "soft_start_duration_ms",
    ] {
        assert!(!saved.contains(removed));
    }
    assert!(load_config(&path).unwrap().motion.calibration_confirmed);
}

#[tokio::test]
async fn removed_motion_json_options_do_not_replace_or_persist_runtime_settings() {
    let directory = tempfile::tempdir().unwrap();
    let config = alia_embedded::config::EmbeddedConfig {
        config_path: directory.path().join("embedded.toml"),
        ..Default::default()
    };
    let manager = alia_embedded::settings::EmbeddedSettingsManager::new(&config);
    manager
        .update_from_value(serde_json::json!({"motion":{"calibration_confirmed":true}}))
        .await
        .unwrap();
    let before = manager.snapshot_json(false).await.unwrap();
    let persisted = std::fs::read(&config.config_path).unwrap();
    for (key, value) in [
        ("reset_on_stop", serde_json::json!(true)),
        ("reset_on_stop", serde_json::json!(false)),
        ("reset_duration_ms", serde_json::json!(1200)),
        ("soft_start_enabled", serde_json::json!(true)),
        ("soft_start_enabled", serde_json::json!(false)),
        ("soft_start_duration_ms", serde_json::json!(1500)),
    ] {
        let mut patch = serde_json::json!({"motion": {}});
        patch["motion"][key] = value;
        assert!(serde_json::from_value::<EmbeddedRuntimeSettings>(patch.clone()).is_err());
        assert!(manager.update_from_json(&patch.to_string()).await.is_err());
        assert_eq!(manager.snapshot_json(false).await.unwrap(), before);
        assert_eq!(std::fs::read(&config.config_path).unwrap(), persisted);
    }
}

#[test]
fn has_vision_debug_defaults() {
    let settings = EmbeddedRuntimeSettings::default();

    assert!(!settings.vision_debug.enabled);
    assert_eq!(settings.vision_debug.bind_addr, "127.0.0.1:50052");
    assert_eq!(settings.vision_debug.stream_fps, 5);
    assert_eq!(settings.vision_debug.jpeg_quality, 75);
    assert!(settings.vision_debug.draw_overlay);
}

#[test]
fn runtime_paths_roundtrip_relative_to_config_directory_and_module_selection_persists() {
    use alia_embedded::config::{load_config, save_runtime_config};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("embedded.toml");
    std::fs::write(
        &path,
        r#"
[device_runtime]
listen_addr = "127.0.0.1:0"
state_path = "isolated/state.sqlite"
plugins_dir = "isolated/plugins"
[modules]
wakeword = "noop"
"#,
    )
    .unwrap();
    let config = load_config(&path).unwrap();
    assert_eq!(config.modules.wakeword, "noop");
    assert_eq!(
        config.device_runtime.state_path,
        dir.path().join("isolated/state.sqlite")
    );
    assert_eq!(
        config.device_runtime.plugins_dir,
        dir.path().join("isolated/plugins")
    );
    assert_eq!(
        config.direct_response_defaults.audio_dir,
        dir.path().join("run/direct-responses")
    );
    save_runtime_config(&config).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("state_path = \"isolated/state.sqlite\""));
    let reloaded = load_config(&path).unwrap();
    assert_eq!(
        reloaded.device_runtime.state_path,
        config.device_runtime.state_path
    );
    assert_eq!(reloaded.modules.wakeword, "noop");
}

#[test]
fn shipped_embedded_example_loads_through_the_runtime_loader() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/embedded.example.toml");
    let config =
        alia_embedded::config::load_config(&path).expect("shipped embedded example must be usable");
    assert_eq!(config.speech.channel_id, "pi-living-room");
}

#[tokio::test]
async fn invalid_voice_instance_ids_do_not_replace_runtime_settings() {
    let dir = tempfile::tempdir().unwrap();
    let config = alia_embedded::config::EmbeddedConfig {
        config_path: dir.path().join("embedded.toml"),
        ..Default::default()
    };
    let manager = alia_embedded::settings::EmbeddedSettingsManager::new(&config);
    manager
        .update_from_value(serde_json::json!({"speech":{"channel_id":"pi-living-room"}}))
        .await
        .unwrap();
    let persisted = std::fs::read(&config.config_path).unwrap();
    for channel_id in [
        "",
        " ",
        " embedded",
        "embedded ",
        "pi living room",
        "pi\nroom",
        "pi\troom",
        "pi\0room",
        "客厅",
    ] {
        let mut speech = config.speech.clone();
        speech.channel_id = channel_id.into();
        assert!(
            speech.validate(&config.audio).is_err(),
            "accepted {channel_id:?}"
        );
        let patch = serde_json::json!({"speech":{"channel_id":channel_id}});
        assert!(serde_json::from_value::<EmbeddedRuntimeSettings>(patch.clone()).is_err());
        assert!(manager.update_from_value(patch).await.is_err());
        assert_eq!(
            manager.snapshot(false).await.settings.speech.channel_id,
            "pi-living-room"
        );
        assert_eq!(std::fs::read(&config.config_path).unwrap(), persisted);
    }
}
