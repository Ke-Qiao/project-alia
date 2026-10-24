//! Explicit local-file inspection, without constructing a runtime or backend.
use std::io::Read;
use std::path::{Path, PathBuf};

use alia_core::embodiment::motion::{MotionAnimationAsset, validate_animation_catalog};
use anyhow::{Context, Result, ensure};
use serde::Serialize;

use crate::config::MotionConfig;

use super::{neck_mapper::NeckMapper, plan, runtime};

#[derive(Debug, Serialize)]
pub struct MotionInspection {
    enabled: bool,
    hardware_enabled: bool,
    calibration_confirmed: bool,
    configured_backend: String,
    servo_count: usize,
    fps: u32,
    center_angle: f32,
    physical_state: &'static str,
    calibration: CalibrationInspection,
    pca_configuration: Option<PcaInspection>,
    approved_animations: Vec<AnimationInspection>,
}

#[derive(Debug, Serialize)]
struct CalibrationInspection {
    path: PathBuf,
    status: &'static str,
    reason: Option<&'static str>,
    servo_count: Option<usize>,
}

#[derive(Debug, Serialize)]
struct PcaInspection {
    device: PathBuf,
    addresses: Vec<u16>,
    frequency_hz: u16,
    min_pulse_us: u16,
    max_pulse_us: u16,
    validation: &'static str,
}

#[derive(Debug, Serialize)]
struct AnimationInspection {
    asset: MotionAnimationAsset,
    source_frames: usize,
}

/// Check the supplied local configuration and approved bytes. This neither
/// approves assets/calibration nor opens a configured device or starts workers.
pub fn inspect_configuration(config: &MotionConfig) -> Result<MotionInspection> {
    validate_animation_catalog(&config.approved_animations).map_err(anyhow::Error::msg)?;
    runtime::validate_config(config)?;
    let is_pca = matches!(config.servo_backend.as_str(), "pca9685" | "i2c");
    if is_pca {
        // Inspect the intended native configuration even while output stays off.
        runtime::validate_pca_addresses(config)?;
    }
    let calibration = inspect_calibration(config)?;
    let mut approved_animations = Vec::with_capacity(config.approved_animations.len());
    if !config.approved_animations.is_empty() {
        let directory = std::fs::symlink_metadata(&config.animation_dir)
            .context("inspecting approved animation directory")?;
        ensure!(
            directory.is_dir(),
            "approved animation directory must be a local directory, not a symlink"
        );
        for asset in &config.approved_animations {
            require_regular(&config.animation_dir.join(&asset.file))?;
            let frames = plan::read_animation(config, asset)?;
            approved_animations.push(AnimationInspection {
                asset: asset.clone(),
                source_frames: frames.len(),
            });
        }
    }
    let pca_configuration = is_pca.then(|| PcaInspection {
        device: config.i2c_device.clone(),
        addresses: config.pca9685_addresses.clone(),
        frequency_hz: config.pca_frequency_hz,
        min_pulse_us: config.min_pulse_us,
        max_pulse_us: config.max_pulse_us,
        validation: "static_rules_only_no_electrical_or_frequency_measurement",
    });
    Ok(MotionInspection {
        enabled: config.enabled,
        hardware_enabled: config.hardware_enabled,
        calibration_confirmed: config.calibration_confirmed,
        configured_backend: config.servo_backend.clone(),
        servo_count: config.servo_count,
        fps: config.fps,
        center_angle: config.center_angle,
        physical_state: "not_observed",
        calibration,
        pca_configuration,
        approved_animations,
    })
}

fn require_regular(path: &Path) -> Result<std::fs::Metadata> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspecting local file {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "inspection input must be a regular local file, not a symlink or special file"
    );
    Ok(metadata)
}

fn inspect_calibration(config: &MotionConfig) -> Result<CalibrationInspection> {
    match std::fs::symlink_metadata(&config.neck_config_file) {
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound && !config.calibration_confirmed =>
        {
            return Ok(CalibrationInspection {
                path: config.neck_config_file.clone(),
                status: "not_checked",
                reason: Some("missing"),
                servo_count: None,
            });
        }
        Err(error) => return Err(error).context("inspecting calibration file"),
        Ok(_) => {}
    }
    require_regular(&config.neck_config_file)?;
    let content = read_calibration(&config.neck_config_file)?;
    let mapper = NeckMapper::from_json(&content)?;
    ensure!(
        mapper.servo_count() == config.servo_count,
        "servo count must match the calibration output count"
    );
    mapper.validate_angles(&vec![config.center_angle; config.servo_count])?;
    Ok(CalibrationInspection {
        path: config.neck_config_file.clone(),
        status: "checked",
        reason: None,
        servo_count: Some(mapper.servo_count()),
    })
}

#[cfg(unix)]
fn read_calibration(path: &Path) -> Result<String> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .context("opening calibration for inspection")?;
    ensure!(
        file.metadata()?.is_file(),
        "calibration must be a regular file"
    );
    let mut content = String::new();
    file.take(1_048_577).read_to_string(&mut content)?;
    ensure!(content.len() <= 1_048_576, "neck calibration exceeds 1 MiB");
    Ok(content)
}

#[cfg(not(unix))]
fn read_calibration(_: &Path) -> Result<String> {
    anyhow::bail!("safe calibration inspection is unavailable on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MATRIX: &str = r#"{
      "ServoMinMax": {"servo_0": [10,170], "servo_1": [10,170]},
      "NeckAngles": [[0,0,0]], "MatrixSize": [1,2], "sigma": 20.0,
      "Matrix": [[90,90]], "NeckAngleMinMax": [-30,30,-30,30,-30,30]
    }"#;

    fn config(root: &Path) -> MotionConfig {
        MotionConfig {
            neck_config_file: root.join("matrix.json"),
            animation_dir: root.join("animations"),
            servo_count: 2,
            servo_backend: "pca9685".into(),
            i2c_device: root.join("must-not-be-opened"),
            ..MotionConfig::default()
        }
    }

    #[test]
    fn missing_unconfirmed_calibration_reports_unchecked_without_creating_files() {
        let root = tempfile::tempdir().unwrap();
        let config = config(root.path());
        let report = inspect_configuration(&config).unwrap();
        assert_eq!(report.calibration.status, "not_checked");
        assert_eq!(report.calibration.reason, Some("missing"));
        assert!(!report.hardware_enabled && !report.calibration_confirmed);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        let mut confirmed = config;
        confirmed.calibration_confirmed = true;
        assert!(inspect_configuration(&confirmed).is_err());
    }

    #[test]
    fn valid_matrix_is_checked_without_claiming_calibration_or_opening_native_device() {
        let root = tempfile::tempdir().unwrap();
        let mut config = config(root.path());
        std::fs::write(&config.neck_config_file, MATRIX).unwrap();
        let report = inspect_configuration(&config).unwrap();
        assert_eq!(report.calibration.status, "checked");
        assert_eq!(report.calibration.servo_count, Some(2));
        assert!(!report.calibration_confirmed);
        config.calibration_confirmed = true;
        config.hardware_enabled = true;
        let report = inspect_configuration(&config).unwrap();
        assert!(report.hardware_enabled && report.calibration_confirmed);
        assert_eq!(report.physical_state, "not_observed");
        assert!(!config.i2c_device.exists());
        assert_eq!(
            std::fs::read_to_string(&config.neck_config_file).unwrap(),
            MATRIX
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn malformed_dimensions_center_pwm_and_confirmation_are_rejected_offline() {
        let root = tempfile::tempdir().unwrap();
        let original = config(root.path());
        std::fs::write(&original.neck_config_file, MATRIX).unwrap();
        for case in 0..8 {
            let mut config = original.clone();
            match case {
                0 => config.servo_count = 3,
                1 => config.center_angle = 5.0,
                2 => config.fps = 0,
                3 => config.min_pulse_us = config.max_pulse_us,
                4 => config.hardware_enabled = true,
                5 => {
                    config.pca9685_addresses = vec![64, 64];
                }
                6 => config.pca9685_addresses.clear(),
                7 => config.pca9685_addresses = vec![0x78],
                _ => unreachable!(),
            }
            assert!(inspect_configuration(&config).is_err(), "case {case}");
        }
        std::fs::write(&original.neck_config_file, "{}").unwrap();
        assert!(inspect_configuration(&original).is_err());
        std::fs::write(&original.neck_config_file, vec![b' '; 1_048_577]).unwrap();
        assert!(inspect_configuration(&original).is_err());
    }

    #[test]
    fn approved_animation_verifies_original_pin_and_production_frame_parser() {
        let root = tempfile::tempdir().unwrap();
        let mut config = config(root.path());
        std::fs::create_dir(&config.animation_dir).unwrap();
        let bytes = b"frames\n1 2 3\n-1 0 2\n";
        let path = config.animation_dir.join("reviewed.anim");
        std::fs::write(&path, bytes).unwrap();
        let asset = MotionAnimationAsset {
            file: "reviewed.anim".into(),
            version: 7,
            sha256: alia_core::embodiment::motion::motion_animation_sha256(bytes),
            bytes: bytes.len() as u32,
        };
        config.approved_animations = vec![asset.clone()];
        let report = inspect_configuration(&config).unwrap();
        assert_eq!(report.approved_animations[0].asset, asset);
        assert_eq!(report.approved_animations[0].source_frames, 2);
        std::fs::write(&path, b"frames\n2 2 3\n-1 0 2\n").unwrap();
        assert!(inspect_configuration(&config).is_err());
        let invalid = b"frames\n1 2 3 4\n";
        std::fs::write(&path, invalid).unwrap();
        config.approved_animations[0].sha256 =
            alia_core::embodiment::motion::motion_animation_sha256(invalid);
        config.approved_animations[0].bytes = invalid.len() as u32;
        assert!(inspect_configuration(&config).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn calibration_and_asset_special_files_are_rejected_before_reading() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let mut config = config(root.path());
        let target = root.path().join("target.json");
        std::fs::write(&target, MATRIX).unwrap();
        symlink(&target, &config.neck_config_file).unwrap();
        assert!(inspect_configuration(&config).is_err());
        std::fs::remove_file(&config.neck_config_file).unwrap();
        std::fs::create_dir(&config.neck_config_file).unwrap();
        assert!(inspect_configuration(&config).is_err());
        std::fs::remove_dir(&config.neck_config_file).unwrap();
        let fifo =
            std::ffi::CString::new(config.neck_config_file.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(inspect_configuration(&config).is_err());
        std::fs::remove_file(&config.neck_config_file).unwrap();

        std::fs::create_dir(&config.animation_dir).unwrap();
        let asset = MotionAnimationAsset {
            file: "reviewed.anim".into(),
            version: 1,
            sha256: alia_core::embodiment::motion::motion_animation_sha256(b"frames\n0 0 0\n"),
            bytes: 13,
        };
        config.approved_animations = vec![asset.clone()];
        let path = config.animation_dir.join(&asset.file);
        symlink(&target, &path).unwrap();
        assert!(inspect_configuration(&config).is_err());
        std::fs::remove_file(&path).unwrap();
        let fifo = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(inspect_configuration(&config).is_err());
    }
}
