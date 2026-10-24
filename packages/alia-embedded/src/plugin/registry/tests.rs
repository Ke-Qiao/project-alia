use super::*;
use std::io::Write;
use std::time::Duration;
use zip::write::SimpleFileOptions;

fn package(script: &str, entry: &str, extra: Option<(&str, &[u8])>) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("manifest.toml", SimpleFileOptions::default())
        .unwrap();
    write!(
        zip,
        "[plugin]\nname = 'probe'\nversion = '1'\ntype = 'sensor'\nruntime = 'trusted_native'\nentry = '{entry}'\n"
    )
    .unwrap();
    zip.start_file(
        "main.py",
        SimpleFileOptions::default().unix_permissions(0o500),
    )
    .unwrap();
    zip.write_all(b"#!/usr/bin/env python3\n").unwrap();
    zip.write_all(script.as_bytes()).unwrap();
    if let Some((name, content)) = extra {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(content).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

const READY: &str = "import os,json,sys,time\ni=os.environ['ALIA_PLUGIN_INSTANCE_ID']\nprint(json.dumps({'event_type':'ready','payload':{'instance_id':i,'protocol_version':1}}),flush=True)\n";

async fn state(registry: &PluginRegistry, expected: PluginState) -> PluginSnapshot {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = registry.list().await.remove(0);
            if snapshot.state == expected {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn install_rejects_traversal_bad_entries_symlinks_and_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plugins");
    let registry = PluginRegistry::new(root.clone());
    let valid = package("pass", "main.py", None);
    for name in ["../escape", "a/b", "", ".", "a\\b"] {
        assert!(registry.install(name, "1", &valid).await.is_err());
    }
    for archive in [
        package("pass", "../main.py", None),
        package("pass", "missing.py", None),
        package("pass", "main.py", Some(("../escape", b"escaped"))),
    ] {
        assert!(registry.install("probe", "1", &archive).await.is_err());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    }
    let mut symlink = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    symlink
        .add_symlink("link", "/tmp", SimpleFileOptions::default())
        .unwrap();
    assert!(
        registry
            .install("probe", "1", &symlink.finish().unwrap().into_inner())
            .await
            .is_err()
    );
    assert!(registry.install("probe", "2", &valid).await.is_err());
    let digest = registry.install("probe", "1", &valid).await.unwrap();
    assert_eq!(digest.len(), 64);
    assert_eq!(registry.list().await[0].state, PluginState::Installed);
    assert!(
        registry
            .install("probe", "1", &package("changed", "main.py", None))
            .await
            .is_err()
    );
    let scanned = PluginRegistry::new(root.clone());
    scanned.scan().await.unwrap();
    assert_eq!(scanned.list().await[0].asset_sha256, digest);
    assert!(!dir.path().join("escape").exists());
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
}

#[tokio::test]
async fn archive_budgets_and_changed_content_fail_before_execution() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("plugins");
    let registry = PluginRegistry::new(root.clone());
    assert!(
        registry
            .install("probe", "1", &vec![0; assets::MAX_ARCHIVE + 1])
            .await
            .is_err()
    );
    let expanded = package(
        "pass",
        "main.py",
        Some(("oversized.bin", &vec![0; 16 * 1024 * 1024 + 1])),
    );
    assert!(registry.install("probe", "1", &expanded).await.is_err());
    registry
        .install("probe", "1", &package("pass", "main.py", None))
        .await
        .unwrap();
    let target = root.join("probe/main.py");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::write(&target, "raise RuntimeError('changed')").unwrap();
    assert!(
        registry
            .start("probe", &registry.list().await[0].asset_sha256, true)
            .await
            .is_err()
    );
    let record_path = root.join("probe/.alia-asset.json");
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    use sha2::{Digest, Sha256};
    record["files"]["main.py"] = serde_json::json!(format!(
        "{:x}",
        Sha256::digest(std::fs::read(&target).unwrap())
    ));
    std::fs::write(record_path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(
        registry
            .start("probe", &registry.list().await[0].asset_sha256, true)
            .await
            .is_err(),
        "rewriting the file manifest must not preserve the old content identity"
    );
    let rescanned = PluginRegistry::new(root);
    rescanned.scan().await.unwrap();
    assert!(rescanned.list().await.is_empty());
}

#[tokio::test]
async fn commands_keep_completion_unknown_and_retry_identity_across_runs() {
    let dir = tempfile::tempdir().unwrap();
    let registry = PluginRegistry::new(dir.path().join("plugins"));
    let effects = dir.path().join("effects.txt");
    let script = format!(
        "{READY}\nfor line in sys.stdin:\n c=json.loads(line)\n with open({},'a') as f: f.write(c['command_id']+'\\n')\n if c['action']=='complete':\n  print(json.dumps({{'event_type':'command_result','payload':{{'instance_id':i,'command_id':c['command_id'],'state':'completed','result':{{'ok':True}}}}}}),flush=True)\n",
        serde_json::to_string(&effects.to_string_lossy()).unwrap()
    );
    registry
        .install("probe", "1", &package(&script, "main.py", None))
        .await
        .unwrap();
    registry
        .start("probe", &registry.list().await[0].asset_sha256, true)
        .await
        .unwrap();
    let first = state(&registry, PluginState::Running).await.instance_id;
    assert!(
        registry
            .start("probe", &registry.list().await[0].asset_sha256, true)
            .await
            .is_err()
    );
    assert!(registry.uninstall("probe").await.is_err());
    let unknown = registry
        .send_command("probe", &first, "lost-ack", "unknown", b"{}")
        .await
        .unwrap();
    assert_eq!(unknown.state, "unknown");
    assert_eq!(
        registry
            .send_command("probe", &first, "lost-ack", "unknown", b"{}")
            .await
            .unwrap()
            .state,
        "unknown"
    );
    assert!(
        registry
            .send_command(
                "probe",
                &first,
                "lost-ack",
                "unknown",
                b"{\"changed\":true}"
            )
            .await
            .is_err()
    );
    registry
        .send_command("probe", &first, "confirmed", "complete", b"{}")
        .await
        .unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let receipt = registry
                .send_command("probe", &first, "confirmed", "complete", b"{}")
                .await
                .unwrap();
            if receipt.state == "completed" {
                break receipt;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&completed.result_json).unwrap(),
        serde_json::json!({"ok":true})
    );
    assert_eq!(
        std::fs::read_to_string(&effects).unwrap(),
        "lost-ack\nconfirmed\n"
    );
    registry.stop("probe").await.unwrap();
    assert!(
        registry
            .start("probe", &registry.list().await[0].asset_sha256, true)
            .await
            .is_err()
    );
    assert!(registry.uninstall("probe").await.is_err());
    state(&registry, PluginState::Stopped).await;
    registry
        .start("probe", &registry.list().await[0].asset_sha256, true)
        .await
        .unwrap();
    let second = state(&registry, PluginState::Running).await.instance_id;
    assert_ne!(first, second);
    assert_eq!(
        registry
            .send_command("probe", &first, "lost-ack", "unknown", b"{}")
            .await
            .unwrap()
            .state,
        "unknown"
    );
    assert!(
        registry
            .send_command("probe", &second, "lost-ack", "unknown", b"{}")
            .await
            .is_err()
    );
    assert!(
        registry
            .send_command("probe", &first, "new-on-old", "unknown", b"{}")
            .await
            .is_err()
    );
    registry.stop("probe").await.unwrap();
    state(&registry, PluginState::Stopped).await;
    registry.uninstall("probe").await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&effects).unwrap(),
        "lost-ack\nconfirmed\n"
    );
}

#[tokio::test]
async fn stop_during_starting_and_oversized_output_join_before_restarting() {
    let dir = tempfile::tempdir().unwrap();
    let registry = PluginRegistry::new(dir.path().join("plugins"));
    registry
        .install(
            "probe",
            "1",
            &package("import time\ntime.sleep(30)\n", "main.py", None),
        )
        .await
        .unwrap();
    registry
        .start("probe", &registry.list().await[0].asset_sha256, true)
        .await
        .unwrap();
    assert_eq!(registry.list().await[0].state, PluginState::Starting);
    registry.stop("probe").await.unwrap();
    state(&registry, PluginState::Stopped).await;
    registry.uninstall("probe").await.unwrap();
    registry
        .install(
            "probe",
            "1",
            &package(
                &format!(
                    "{READY}\nsys.stdout.write('x'*70000)\nsys.stdout.flush()\ntime.sleep(30)\n"
                ),
                "main.py",
                None,
            ),
        )
        .await
        .unwrap();
    registry
        .start("probe", &registry.list().await[0].asset_sha256, true)
        .await
        .unwrap();
    state(&registry, PluginState::Failed).await;
    registry.uninstall("probe").await.unwrap();
}

#[tokio::test]
async fn child_exit_cleans_descendant_pipes_before_releasing_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let registry = PluginRegistry::new(dir.path().join("plugins"));
    let script = format!("{READY}\nimport subprocess\nsubprocess.Popen(['sleep','30'])\n");
    registry
        .install("probe", "1", &package(&script, "main.py", None))
        .await
        .unwrap();
    registry
        .start("probe", &registry.list().await[0].asset_sha256, true)
        .await
        .unwrap();
    // A surviving descendant would keep both pipes open for thirty seconds.
    state(&registry, PluginState::Stopped).await;
    assert!(registry.list().await[0].instance_id.is_empty());
    registry.uninstall("probe").await.unwrap();
}

#[tokio::test]
async fn forged_completion_cannot_resolve_the_original_unknown_command() {
    let dir = tempfile::tempdir().unwrap();
    let registry = PluginRegistry::new(dir.path().join("plugins"));
    let script = format!(
        "{READY}\nfor line in sys.stdin:\n c=json.loads(line)\n print(json.dumps({{'event_type':'command_result','payload':{{'instance_id':'another-run','command_id':c['command_id'],'state':'completed'}}}}),flush=True)\n"
    );
    registry
        .install("probe", "1", &package(&script, "main.py", None))
        .await
        .unwrap();
    registry
        .start("probe", &registry.list().await[0].asset_sha256, true)
        .await
        .unwrap();
    let instance = state(&registry, PluginState::Running).await.instance_id;
    registry
        .send_command("probe", &instance, "original", "effect", b"{}")
        .await
        .unwrap();
    state(&registry, PluginState::Failed).await;
    assert_eq!(
        registry
            .send_command("probe", &instance, "original", "effect", b"{}")
            .await
            .unwrap()
            .state,
        "unknown"
    );
    registry.uninstall("probe").await.unwrap();
}

#[tokio::test]
async fn rust_wasm_processor_requires_reviewed_asset_and_records_completion() {
    let module_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../examples/wasm-plugins/target/wasm32-unknown-unknown/release/alia_wasm_demo.wasm",
    );
    let module =
        std::fs::read(module_path).expect("build examples/wasm-plugins with Rust 1.96 first");
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("manifest.toml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"[plugin]\nname='probe'\nversion='1'\ntype='processor'\nruntime='wasm'\nentry='module.wasm'\n[plugin.wasm_limits]\nmemory_pages=256\nfuel=20000000\ninitialization_fuel=2000000\ntimeout_ms=10000\nmax_input_bytes=65536\nmax_output_bytes=65536\n").unwrap();
    zip.start_file("module.wasm", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&module).unwrap();
    let archive = zip.finish().unwrap().into_inner();
    let dir = tempfile::tempdir().unwrap();
    let registry = PluginRegistry::new(dir.path().join("plugins"));
    let digest = registry.install("probe", "1", &archive).await.unwrap();
    assert!(registry.start("probe", "wrong", false).await.is_err());
    registry.start("probe", &digest, false).await.unwrap();
    let instance = state(&registry, PluginState::Running).await.instance_id;
    registry
        .send_command("probe", &instance, "echo-1", "echo", br#"{"value":42}"#)
        .await
        .unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let receipt = registry
                .send_command("probe", &instance, "echo-1", "echo", br#"{"value":42}"#)
                .await
                .unwrap();
            if receipt.state != "unknown" {
                break receipt;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(receipt.state, "completed");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&receipt.result_json).unwrap(),
        serde_json::json!({"value":42})
    );
    registry.stop("probe").await.unwrap();
    state(&registry, PluginState::Stopped).await;
    registry.uninstall("probe").await.unwrap();
}

#[tokio::test]
async fn native_device_mode_requires_separate_operator_authorization() {
    let dir = tempfile::tempdir().unwrap();
    let registry = PluginRegistry::new(dir.path().join("plugins"));
    let digest = registry
        .install("probe", "1", &package("pass", "main.py", None))
        .await
        .unwrap();
    assert!(
        registry
            .start("probe", &digest, false)
            .await
            .unwrap_err()
            .to_string()
            .contains("explicit operator authorization")
    );
    assert_eq!(registry.list().await[0].state, PluginState::Installed);
}
