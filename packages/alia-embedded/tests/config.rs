use std::path::{Path, PathBuf};

use alia_embedded::config::{
    EmbeddedRuntimeSettings, normalize_config_path_for_cwd, relativize_runtime_paths,
};

#[test]
fn silero_configuration_rejects_removed_limits_and_unsupported_formats() {
    for old in ["max_speech_duration_ms = 15000", "buffer_size_sec = 10.0"] {
        let text = format!("[vad]\n{old}\n");
        assert!(toml::from_str::<EmbeddedRuntimeSettings>(&text).is_err());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embedded.toml");
        std::fs::write(&path, text).unwrap();
        assert!(alia_embedded::config::load_config(&path).is_err());
    }
    for invalid in [
        "sample_rate = 8000",
        "window_size = 256",
        "provider = 'cuda'",
        "num_threads = 0",
        "threshold = nan",
        "threshold = 1.0",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embedded.toml");
        std::fs::write(&path, format!("[vad]\n{invalid}\n")).unwrap();
        assert!(
            alia_embedded::config::load_config(&path).is_err(),
            "{invalid}"
        );
    }
    assert_eq!(EmbeddedRuntimeSettings::default().modules.vad, "silero");
}

#[tokio::test]
async fn invalid_vad_update_preserves_memory_and_disk() {
    use alia_embedded::config::{load_config, save_runtime_config};
    use alia_embedded::settings::EmbeddedSettingsManager;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("embedded.toml");
    let config = load_config(&path).unwrap();
    save_runtime_config(&config).unwrap();
    let manager = EmbeddedSettingsManager::new(&config);
    let before_memory = manager.snapshot_json(false).await.unwrap();
    let before_disk = std::fs::read(&path).unwrap();
    for patch in [
        serde_json::json!({"vad":{"max_speech_duration_ms":15000}}),
        serde_json::json!({"vad":{"buffer_size_sec":10.0}}),
        serde_json::json!({"vad":{"provider":"cuda"}}),
    ] {
        assert!(manager.update_from_value(patch).await.is_err());
        assert_eq!(manager.snapshot_json(false).await.unwrap(), before_memory);
        assert_eq!(std::fs::read(&path).unwrap(), before_disk);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_settings_patches_and_device_id_keep_all_committed_changes() {
    use alia_embedded::config::{EmbeddedConfig, load_config};
    use alia_embedded::settings::EmbeddedSettingsManager;
    use std::sync::Arc;
    let directory = tempfile::tempdir().unwrap();
    let config = EmbeddedConfig {
        config_path: directory.path().join("embedded.toml"),
        ..Default::default()
    };
    let manager = Arc::new(EmbeddedSettingsManager::new(&config));
    let gate = Arc::new(tokio::sync::Barrier::new(3));
    let mut tasks = Vec::new();
    for patch in [
        serde_json::json!({"identity":{"device_name":"updated-name"}}),
        serde_json::json!({"motion":{"fps":60}}),
    ] {
        let manager = manager.clone();
        let gate = gate.clone();
        tasks.push(tokio::spawn(async move {
            gate.wait().await;
            manager.update_from_value(patch).await.unwrap();
        }));
    }
    gate.wait().await;
    assert!(manager.ensure_device_id("new-device").await.unwrap());
    for task in tasks {
        task.await.unwrap();
    }
    let snapshot = manager.snapshot(false).await;
    let persisted = load_config(&config.config_path).unwrap();
    for settings in [snapshot.settings, persisted.runtime_settings()] {
        assert_eq!(settings.identity.device_name, "updated-name");
        assert_eq!(settings.identity.device_id, "new-device");
        assert_eq!(settings.motion.fps, 60);
    }
    assert!(!manager.ensure_device_id("replacement").await.unwrap());
}

#[tokio::test]
async fn failed_device_id_save_keeps_memory_unchanged_and_allows_retry() {
    use alia_embedded::config::EmbeddedConfig;
    use alia_embedded::settings::EmbeddedSettingsManager;
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("embedded.toml");
    std::fs::create_dir(&target).unwrap();
    let manager = EmbeddedSettingsManager::new(&EmbeddedConfig {
        config_path: target.clone(),
        ..Default::default()
    });
    assert!(manager.ensure_device_id("new-device").await.is_err());
    assert!(
        manager
            .snapshot(false)
            .await
            .settings
            .identity
            .device_id
            .is_empty()
    );
    std::fs::remove_dir(&target).unwrap();
    assert!(manager.ensure_device_id("new-device").await.unwrap());
    assert_eq!(
        manager.snapshot(false).await.settings.identity.device_id,
        "new-device"
    );
}

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
    assert!(settings.motion.approved_animations.is_empty());
    assert_eq!(settings.motion.max_servo_speed_deg_per_sec, 60.0);
}

fn reviewed_fixture_asset() -> alia_core::embodiment::motion::MotionAnimationAsset {
    let bytes = b"frames\n1 0 0\n";
    alia_core::embodiment::motion::MotionAnimationAsset {
        file: "reviewed.anim".into(),
        version: 1,
        sha256: alia_core::embodiment::motion::motion_animation_sha256(bytes),
        bytes: bytes.len() as u32,
    }
}

#[test]
fn local_animation_catalog_roundtrips_and_invalid_catalogs_are_rejected_before_save() {
    use alia_embedded::config::{EmbeddedConfig, load_config, save_runtime_config};
    let directory = tempfile::tempdir().unwrap();
    let mut config = EmbeddedConfig {
        config_path: directory.path().join("embedded.toml"),
        ..Default::default()
    };
    config.motion.approved_animations = vec![reviewed_fixture_asset()];
    save_runtime_config(&config).unwrap();
    assert_eq!(
        load_config(&config.config_path)
            .unwrap()
            .motion
            .approved_animations,
        config.motion.approved_animations
    );
    let valid = std::fs::read(&config.config_path).unwrap();
    for invalid in [
        vec![reviewed_fixture_asset(); 2],
        vec![alia_core::embodiment::motion::MotionAnimationAsset {
            version: 0,
            ..reviewed_fixture_asset()
        }],
        vec![reviewed_fixture_asset(); 65],
    ] {
        config.motion.approved_animations = invalid;
        assert!(save_runtime_config(&config).is_err());
        assert_eq!(std::fs::read(&config.config_path).unwrap(), valid);
        let unchecked = toml::to_string(&config.runtime_settings()).unwrap();
        std::fs::write(&config.config_path, unchecked).unwrap();
        assert!(load_config(&config.config_path).is_err());
        std::fs::write(&config.config_path, &valid).unwrap();
        config.motion.enabled = false;
        assert!(alia_embedded::module::motion::MotionRuntime::new(config.motion.clone()).is_err());
    }
}

#[tokio::test]
async fn remote_settings_cannot_create_replace_or_clear_the_local_animation_catalog() {
    use alia_embedded::config::{EmbeddedConfig, save_runtime_config};
    use alia_embedded::settings::EmbeddedSettingsManager;
    let directory = tempfile::tempdir().unwrap();
    let mut config = EmbeddedConfig {
        config_path: directory.path().join("embedded.toml"),
        ..Default::default()
    };
    let unapproved = EmbeddedSettingsManager::new(&config);
    assert!(
        unapproved
            .update_from_value(
                serde_json::json!({"motion":{"approved_animations":[reviewed_fixture_asset()]}})
            )
            .await
            .is_err()
    );
    assert!(!config.config_path.exists());
    config.motion.approved_animations = vec![reviewed_fixture_asset()];
    save_runtime_config(&config).unwrap();
    let manager = EmbeddedSettingsManager::new(&config);
    // Unchanged complete snapshots remain compatible; local trust does not change.
    manager
        .update_from_json(&manager.snapshot_json(false).await.unwrap())
        .await
        .unwrap();
    manager
        .update_from_value(serde_json::json!({"motion":{"fps":60}}))
        .await
        .unwrap();
    let before = manager.snapshot_json(false).await.unwrap();
    let persisted = std::fs::read(&config.config_path).unwrap();
    let mut changed = reviewed_fixture_asset();
    changed.version = 2;
    let mut whole = serde_json::from_str::<serde_json::Value>(&before).unwrap();
    whole["motion"]["approved_animations"] = serde_json::json!([changed.clone()]);
    for patch in [
        serde_json::json!({"motion":{"approved_animations":[]}}),
        serde_json::json!({"motion":{"approved_animations":[changed]}}),
        whole,
        serde_json::json!({"motion":null}),
    ] {
        assert!(manager.update_from_value(patch).await.is_err());
        assert_eq!(manager.snapshot_json(false).await.unwrap(), before);
        assert_eq!(std::fs::read(&config.config_path).unwrap(), persisted);
    }
}

#[tokio::test]
async fn cue_upload_directory_uses_the_resolved_startup_snapshot() {
    use alia_embedded::config::load_config;
    use alia_embedded::settings::EmbeddedSettingsManager;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("embedded.toml");
    std::fs::write(
        &path,
        "[direct_response_defaults]\naudio_dir = 'isolated/cues'\n",
    )
    .unwrap();
    let mut config = load_config(&path).unwrap();
    let resolved = directory.path().join("isolated/cues");
    let manager = EmbeddedSettingsManager::new(&config);
    assert_eq!(manager.direct_response_audio_dir(), resolved);
    assert!(manager.direct_response_audio_dir().is_absolute());
    assert_eq!(
        manager
            .snapshot(false)
            .await
            .settings
            .direct_response_defaults
            .audio_dir,
        PathBuf::from("isolated/cues")
    );
    // A caller changing its config after construction cannot redirect the owner.
    config.direct_response_defaults.audio_dir = directory.path().join("changed");
    manager
        .update_from_json(&manager.snapshot_json(false).await.unwrap())
        .await
        .unwrap();
    manager
        .update_from_value(serde_json::json!({"motion":{"fps":60}}))
        .await
        .unwrap();
    assert_eq!(manager.direct_response_audio_dir(), resolved);
    assert_eq!(manager.snapshot(false).await.settings.motion.fps, 60);
    assert_eq!(
        load_config(&path)
            .unwrap()
            .direct_response_defaults
            .audio_dir,
        resolved
    );
}

#[tokio::test]
async fn remote_cue_directory_changes_preserve_memory_disk_and_startup_owner() {
    use alia_embedded::config::{load_config, save_runtime_config};
    use alia_embedded::settings::EmbeddedSettingsManager;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("embedded.toml");
    let mut config = load_config(&path).unwrap();
    config.direct_response_defaults.audio_dir = directory.path().join("isolated/cues");
    let manager = EmbeddedSettingsManager::new(&config);
    let redirect = serde_json::json!({"direct_response_defaults":{"audio_dir":"elsewhere"}});
    assert!(manager.update_from_value(redirect.clone()).await.is_err());
    assert!(!path.exists(), "a refused update must not create a config");
    save_runtime_config(&config).unwrap();
    let before = manager.snapshot_json(false).await.unwrap();
    let persisted = std::fs::read(&path).unwrap();
    let mut whole = serde_json::from_str::<serde_json::Value>(&before).unwrap();
    whole["direct_response_defaults"]["audio_dir"] = serde_json::json!("elsewhere");
    for patch in [
        redirect,
        // Even equivalent path spellings must not replace the local raw value.
        serde_json::json!({"direct_response_defaults":{"audio_dir":"./isolated/cues"}}),
        serde_json::json!({"direct_response_defaults":{"audio_dir":"isolated/./cues"}}),
        serde_json::json!({"direct_response_defaults":{"audio_dir":config.direct_response_defaults.audio_dir}}),
        serde_json::json!({"direct_response_defaults":{"audio_dir":""}}),
        serde_json::json!({"direct_response_defaults":{"audio_dir":null}}),
        serde_json::json!({"direct_response_defaults":null}),
        whole,
    ] {
        assert!(manager.update_from_value(patch).await.is_err());
        assert_eq!(manager.snapshot_json(false).await.unwrap(), before);
        assert_eq!(std::fs::read(&path).unwrap(), persisted);
        assert_eq!(
            manager.direct_response_audio_dir(),
            config.direct_response_defaults.audio_dir
        );
    }
    manager
        .update_from_value(
            serde_json::json!({"direct_response_defaults":{"audio_dir":"isolated/cues"}}),
        )
        .await
        .unwrap();
    assert_eq!(manager.snapshot_json(false).await.unwrap(), before);
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

#[test]
fn r1_requires_explicit_path_without_changing_existing_defaults() {
    use alia_embedded::config::{load_config, save_runtime_settings};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("embedded.toml");
    let settings = EmbeddedRuntimeSettings::default();
    assert!(settings.r1.serial_path.as_os_str().is_empty());
    assert_ne!(settings.modules.wakeword, "r1");
    save_runtime_settings(&path, &settings).unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut invalid = settings;
    invalid.modules.wakeword = "r1".into();
    assert!(save_runtime_settings(&path, &invalid).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::write(&path, "[modules]\nwakeword = 'r1'\n").unwrap();
    assert!(load_config(&path).is_err());
}

#[test]
fn r1_path_roundtrip_and_preflight_do_not_open_the_control_device() {
    use alia_embedded::config::{load_config, save_runtime_config};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("embedded.toml");
    std::fs::write(
        &path,
        "[modules]\nwakeword = 'r1'\n[r1]\nserial_path = 'control-port'\n[motion]\nenabled = false\nhardware_enabled = false\n",
    )
    .unwrap();
    let config = load_config(&path).unwrap();
    assert_eq!(config.r1.serial_path, dir.path().join("control-port"));
    save_runtime_config(&config).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("serial_path = \"control-port\""));
    let report = alia_embedded::preflight::check_config(&path).unwrap();
    assert_eq!(report["r1_control"]["selected"], true);
    assert_eq!(report["r1_control"]["device_opened"], false);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert_eq!(
        load_config(&path).unwrap().r1.serial_path,
        config.r1.serial_path
    );
}

#[test]
fn r1_preset_uses_explicit_duplex_audio_with_independent_clocks() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/embedded.r1.example.toml");
    let config = alia_embedded::config::load_config(&path).unwrap();
    assert_eq!(config.modules.wakeword, "r1");
    assert_eq!(config.modules.audio_capture, "cpal");
    assert_eq!(config.modules.audio_playback, "cpal");
    assert_eq!(config.audio.capture_device, "hw:CARD=L6Microphone,DEV=0");
    assert_eq!(
        config.audio.capture_backend,
        alia_embedded::config::AudioBackend::Alsa
    );
    assert_eq!(config.audio.capture_sample_rate, 16000);
    assert_eq!(config.audio.capture_channels, 1);
    assert_eq!(config.audio.capture_volume, 1.0);
    assert_eq!(config.audio.playback_device, "hw:CARD=Device,DEV=0");
    assert_eq!(
        config.audio.playback_backend,
        alia_embedded::config::AudioBackend::Alsa
    );
    assert_eq!(config.audio.playback_sample_rate, 48000);
    assert_eq!(config.audio.playback_channels, 2);
    assert_eq!(config.audio.playback_volume, 1.0);
    assert!(!config.audio.alsa_playback_lock_to_capture_rate);
    assert!(config.speech.validate(&config.audio).is_ok());
    assert!(!config.motion.hardware_enabled && !config.motion.enabled);
    assert!(!config.dev.skip_binding);
    for unsupported in ["beam = 0", "manual_wakeup = true", "aec = true"] {
        assert!(
            toml::from_str::<EmbeddedRuntimeSettings>(&format!("[r1]\n{unsupported}")).is_err()
        );
    }
}

#[test]
fn system_capture_is_explicit_and_does_not_enable_uart_commands_in_config() {
    use alia_embedded::config::SpeechTrigger;
    let defaults: EmbeddedRuntimeSettings = toml::from_str("").unwrap();
    assert_eq!(defaults.speech.trigger, SpeechTrigger::Wakeword);
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/embedded.r1-system.example.toml");
    let config = alia_embedded::config::load_config(&path).unwrap();
    assert_eq!(config.speech.trigger, SpeechTrigger::System);
    assert_eq!(config.modules.wakeword, "noop");
    assert_eq!(config.audio.capture_device, "hw:CARD=L6Microphone,DEV=0");
    assert_eq!(config.audio.capture_sample_rate, 16000);
    assert_eq!(config.audio.capture_channels, 1);
    assert!(config.speech.validate(&config.audio).is_ok());
    let saved = toml::to_string(&config.runtime_settings()).unwrap();
    let loaded: EmbeddedRuntimeSettings = toml::from_str(&saved).unwrap();
    assert_eq!(loaded.speech.trigger, SpeechTrigger::System);
    let report = alia_embedded::preflight::check_config(&path).unwrap();
    assert_eq!(report["speech_trigger"], "system");
    assert_eq!(report["r1_control"]["device_opened"], false);
    assert!(toml::from_str::<EmbeddedRuntimeSettings>("[speech]\ntrigger='auto'\n").is_err());
    assert!(toml::from_str::<EmbeddedRuntimeSettings>("[r1]\nmanual_wakeup=true\n").is_err());
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
