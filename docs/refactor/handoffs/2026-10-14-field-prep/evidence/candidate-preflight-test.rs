use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

const STAGING: &str = include_str!("../../../config/embedded.staging.example.toml");

fn guarded_command(root: &Path) -> Command {
    // A CLI regression must fail at identity creation before any device startup.
    std::fs::write(root.join("blocker"), b"not a directory").unwrap();
    std::fs::write(
        root.join("guard.toml"),
        STAGING.replace("staging/identity/device.key", "blocker/device.key"),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_alia-embedded"));
    command
        .current_dir(root)
        .env("ALIA_EMBEDDED_CONFIG_PATH", root.join("guard.toml"))
        .env_remove("ALIA_EMBEDDED_CHECK_KWS_WAV");
    command
}

fn report(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn snapshot(root: &Path) -> Vec<(std::ffi::OsString, Vec<u8>, bool)> {
    let mut files: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            assert!(metadata.is_file(), "unexpected directory or special file");
            (
                entry.file_name(),
                std::fs::read(entry.path()).unwrap(),
                metadata.permissions().readonly(),
            )
        })
        .collect();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

#[test]
fn check_precedes_runtime_and_kws_and_preserves_files_and_key_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut command = guarded_command(&root);
    let content =
        "[identity]\nkeypair_path = 'existing.key'\n[speech]\nchannel_id = 'field-check'\n";
    std::fs::write(root.join("input.toml"), content).unwrap();
    std::fs::write(root.join("existing.key"), b"not a runtime key").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.join("existing.key"),
            std::fs::Permissions::from_mode(0o640),
        )
        .unwrap();
    }
    let before = snapshot(&root);
    let result = report(
        command
            .env("ALIA_EMBEDDED_CHECK_KWS_WAV", root.join("missing.wav"))
            .args(["--check-config", "input.toml"])
            .output()
            .unwrap(),
    );
    assert_eq!(result["mode"], "offline_configuration_check");
    assert_eq!(result["hardware_verified"], false);
    assert_eq!(result["speech_channel_id"], "field-check");
    assert_eq!(result["configured_modules"]["audio_capture"], "cpal");
    assert_eq!(
        result["configured_runtime"]["keypair_path"],
        root.join("existing.key").to_str().unwrap()
    );
    assert_eq!(result["motion"]["calibration"]["status"], "not_checked");
    assert_eq!(result["motion"]["physical_state"], "not_observed");
    assert_eq!(snapshot(&root), before);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.join("existing.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }
}

#[test]
fn staging_profile_checks_without_creating_runtime_directories() {
    let dir = tempfile::tempdir().unwrap();
    let mut command = guarded_command(dir.path());
    std::fs::write(dir.path().join("staging.toml"), STAGING).unwrap();
    let before = snapshot(dir.path());
    let result = report(
        command
            .args(["--check-config", "staging.toml"])
            .output()
            .unwrap(),
    );
    assert!(
        result["configured_modules"]
            .as_object()
            .unwrap()
            .values()
            .all(|v| v == "noop")
    );
    assert_eq!(result["configured_runtime"]["listen_addr"], "127.0.0.1:0");
    assert_eq!(result["configured_runtime"]["skip_binding"], false);
    for field in ["enabled", "hardware_enabled", "calibration_confirmed"] {
        assert_eq!(result["motion"][field], false);
    }
    for field in [
        "vision_enabled",
        "vision_debug_enabled",
        "hmmd_enabled",
        "ble_provision_enabled",
    ] {
        assert_eq!(result["configured_runtime"][field], false);
    }
    assert_eq!(snapshot(dir.path()), before);
}

#[test]
fn missing_invalid_obsolete_and_oversized_configs_fail_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    for (content, expected) in [
        (None, "checking existing configuration"),
        (
            Some("not = [".to_string()),
            "parsing embedded configuration",
        ),
        (
            Some("[speech]\nenabled = true".to_string()),
            "unknown field",
        ),
        (Some("#".repeat(1_048_577)), "exceeds inspection limit"),
        (Some("[speech]\nchannel_id = ''".to_string()), "channel_id"),
    ] {
        let mut command = guarded_command(dir.path());
        let path = dir.path().join("input.toml");
        if let Some(content) = content {
            std::fs::write(&path, content).unwrap();
        }
        let before = snapshot(dir.path());
        let output = command
            .args(["--check-config", "input.toml"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(snapshot(dir.path()), before);
    }
}

#[test]
fn help_and_invalid_arguments_never_enter_normal_startup() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["--help"],
        vec!["-h"],
        vec!["--check-config"],
        vec!["--check-confg", "missing.toml"],
        vec!["--check-config", "a", "b"],
    ] {
        let mut command = guarded_command(dir.path());
        let before = snapshot(dir.path());
        let output = command.args(&args).output().unwrap();
        if args[0] == "--help" || args[0] == "-h" {
            assert!(output.status.success());
            assert!(String::from_utf8_lossy(&output.stdout).contains("--check-config"));
        } else {
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("expected no arguments"));
        }
        assert_eq!(snapshot(dir.path()), before);
    }
}

#[cfg(unix)]
#[test]
fn symlink_directory_and_fifo_inputs_are_rejected_before_opening() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("real.toml"), STAGING).unwrap();
    std::os::unix::fs::symlink("real.toml", dir.path().join("link.toml")).unwrap();
    std::fs::create_dir(dir.path().join("directory")).unwrap();
    let fifo =
        std::ffi::CString::new(dir.path().join("fifo").as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    for name in ["link.toml", "directory", "fifo"] {
        let output = guarded_command(dir.path())
            .args(["--check-config", name])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("must be a regular file"));
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 6);
}
