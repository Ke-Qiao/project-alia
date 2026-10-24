//! Offline configuration inspection. No runtime, identity, model or device is constructed.
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::config::{self, EmbeddedConfig, EmbeddedRuntimeSettings};

pub fn check_config(path: &Path) -> Result<Value> {
    let path = std::path::absolute(path).context("resolving configuration path")?;
    let metadata = std::fs::symlink_metadata(&path)
        .with_context(|| format!("checking existing configuration {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "configuration must be a regular file, not a symlink or device"
    );
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options
        .open(&path)
        .context("opening configuration for inspection")?;
    ensure!(
        file.metadata()?.is_file(),
        "configuration must be a regular file"
    );
    let mut content = String::new();
    file.take(1_048_577).read_to_string(&mut content)?;
    ensure!(
        content.len() <= 1_048_576,
        "configuration exceeds inspection limit of 1 MiB"
    );
    let settings: EmbeddedRuntimeSettings =
        toml::from_str(&content).context("parsing embedded configuration for inspection")?;
    let mut config = EmbeddedConfig::default();
    config.config_path = path;
    config.apply_runtime_settings(settings);
    // Reuse the runtime's typed schema, path resolution and validation without building a node.
    config::resolve_config_paths(&mut config)?;
    config.speech.validate(&config.audio)?;
    let motion = crate::module::motion::inspect_configuration(&config.motion)?;

    Ok(json!({
        "mode": "offline_configuration_check",
        "config_path": config.config_path,
        "hardware_verified": false,
        "checks": ["typed_configuration", "device_runtime_bounds", "speech_contract", "motion_configuration_and_files"],
        "not_checked": ["module_instantiation", "model_loading", "device_enumeration_and_permissions", "database_or_network", "server_catalog_agreement", "physical_calibration", "aec_and_acoustics"],
        "build_features": {
            "sherpa_onnx": cfg!(feature = "sherpa-onnx"),
            "silero": cfg!(feature = "sherpa-onnx"),
            "vision_opencv": cfg!(feature = "vision-opencv"),
        },
        "configured_modules": config.modules,
        "r1_control": {
            "selected": config.modules.wakeword == "r1",
            "serial_path": config.r1.serial_path,
            "baud_rate": 115200,
            "target_supported": cfg!(target_os = "linux"),
            "device_opened": false,
        },
        "speech_channel_id": config.speech.channel_id,
        "speech_trigger": config.speech.trigger,
        "configured_audio": {
            "capture_backend": config.audio.capture_backend,
            "capture_device": config.audio.capture_device,
            "capture_sample_rate": config.audio.capture_sample_rate,
            "capture_channels": config.audio.capture_channels,
            "playback_backend": config.audio.playback_backend,
            "playback_device": config.audio.playback_device,
            "playback_sample_rate": config.audio.playback_sample_rate,
            "playback_channels": config.audio.playback_channels,
            "alsa_playback_lock_to_capture_rate": config.audio.alsa_playback_lock_to_capture_rate,
        },
        "configured_runtime": {
            "listen_addr": config.device_runtime.listen_addr,
            "state_path": config.device_runtime.state_path,
            "plugins_dir": config.device_runtime.plugins_dir,
            "keypair_path": config.identity.keypair_path,
            "cue_directory": config.direct_response_defaults.audio_dir,
            "skip_binding": config.dev.skip_binding,
            "vision_enabled": config.vision.enabled,
            "vision_debug_enabled": config.vision_debug.enabled,
            "hmmd_enabled": config.hmmd_mmwave.enabled,
            "ble_provision_enabled": config.discovery.ble_provision_enabled,
        },
        "motion": motion,
    }))
}
