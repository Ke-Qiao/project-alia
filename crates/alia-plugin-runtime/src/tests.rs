use super::*;
use crate::package::{PackageManifest, PackageStore};

fn binary(wat: &str) -> Vec<u8> {
    wat::parse_str(wat).unwrap()
}
fn echo() -> Vec<u8> {
    binary(
        r#"(module
    (import "extism:host/env" "input_offset" (func $input (result i64)))
    (import "extism:host/env" "input_length" (func $length (result i64)))
    (import "extism:host/env" "output_set" (func $output (param i64 i64)))
    (func (export "echo") (result i32)
        (call $output (call $input) (call $length)) (i32.const 0)))"#,
    )
}
fn enabled(module: Vec<u8>, limits: Limits) -> Arc<Deployment> {
    let deployment = Deployment::new(module, limits, &serde_json::json!({"test":1})).unwrap();
    deployment.enable(deployment.implementation_pin()).unwrap();
    deployment
}
async fn call(deployment: &Deployment, export: &str, input: &[u8]) -> Result<Vec<u8>> {
    deployment
        .invoke(
            export,
            input,
            Instant::now() + Duration::from_secs(10),
            CancellationToken::new(),
        )
        .await
}
fn limits() -> Limits {
    Limits {
        timeout_ms: 10_000,
        ..Limits::default()
    }
}

#[test]
fn rejects_ambient_authority_unbounded_memory_and_start_sections() {
    for module in [
        "(module (import \"wasi_snapshot_preview1\" \"fd_write\" (func)))",
        "(module (import \"extism:host/user\" \"execute\" (func)))",
        "(module (import \"extism:host/env\" \"http_request\" (func)))",
        "(module (memory 1))",
        "(module (table 1 funcref))",
        "(module (func $start) (start $start))",
    ] {
        assert!(
            Deployment::new(binary(module), limits(), &serde_json::Value::Null).is_err(),
            "{module}"
        );
    }
}

#[test]
fn export_admission_rejects_missing_non_function_and_wrong_signatures() {
    let deployment = Deployment::new(
        binary(
            r#"(module
        (import "extism:host/env" "input_length" (func (result i64)))
        (func (export "ok") (result i32) (i32.const 0))
        (func (export "empty"))
        (func (export "argument") (param i32))
        (func (export "wrong_result") (result i64) (i64.const 0))
        (memory (export "memory") 1 1))"#,
        ),
        limits(),
        &serde_json::Value::Null,
    )
    .unwrap();
    deployment.require_export("ok").unwrap();
    deployment.require_export("empty").unwrap();
    for name in ["missing", "memory", "argument", "wrong_result"] {
        assert!(deployment.require_export(name).is_err(), "{name}");
    }
}

#[tokio::test]
async fn revocation_rejects_stale_arcs_and_host_pin_covers_metadata() {
    let deployment = enabled(echo(), limits());
    assert_eq!(
        call(&deployment, "echo", b"private-data").await.unwrap(),
        b"private-data"
    );
    let changed = Deployment::new(echo(), limits(), &serde_json::json!({"test":2})).unwrap();
    assert_ne!(
        deployment.implementation_pin(),
        changed.implementation_pin()
    );
    deployment.revoke().await.unwrap();
    assert!(call(&deployment, "echo", b"input").await.is_err());
    assert!(deployment.enable(deployment.implementation_pin()).is_err());
}

#[tokio::test]
async fn every_invocation_has_fresh_linear_state() {
    let module = binary(
        r#"(module
        (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
        (import "extism:host/env" "store_u64" (func $store (param i64 i64)))
        (import "extism:host/env" "output_set" (func $output (param i64 i64)))
        (global $counter (mut i64) (i64.const 0))
        (func (export "count") (result i32) (local $ptr i64)
            (global.set $counter (i64.add (global.get $counter) (i64.const 1)))
            (local.set $ptr (call $alloc (i64.const 8)))
            (call $store (local.get $ptr) (global.get $counter))
            (call $output (local.get $ptr) (i64.const 8)) (i32.const 0)))"#,
    );
    let deployment = enabled(module, limits());
    for _ in 0..2 {
        assert_eq!(
            call(&deployment, "count", b"").await.unwrap(),
            1u64.to_le_bytes()
        );
    }
    deployment.shutdown().await.unwrap();
}

#[tokio::test]
async fn timeout_retains_cleanup_owner_and_does_not_cancel_another_engine() {
    let looping = enabled(
        binary("(module (func (export \"spin\") (result i32) (loop $l (br $l)) (i32.const 0)))"),
        limits(),
    );
    let healthy = enabled(echo(), limits());
    let timed = looping.invoke(
        "spin",
        b"",
        Instant::now() + Duration::from_millis(2),
        CancellationToken::new(),
    );
    let good = call(&healthy, "echo", b"next-owner");
    let (timed, good) = tokio::join!(timed, good);
    assert!(timed.is_err());
    assert_eq!(good.unwrap(), b"next-owner");
    tokio::time::timeout(Duration::from_secs(10), looping.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        call(&healthy, "echo", b"still-healthy").await.unwrap(),
        b"still-healthy"
    );
    healthy.shutdown().await.unwrap();
}

#[tokio::test]
async fn oversized_output_and_guest_error_are_bounded_before_copy() {
    let huge_output = binary(
        r#"(module
        (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
        (import "extism:host/env" "output_set" (func $output (param i64 i64)))
        (func (export "huge") (result i32)
            (call $output (call $alloc (i64.const 4096)) (i64.const 4096)) (i32.const 0)))"#,
    );
    let deployment = enabled(
        huge_output,
        Limits {
            max_output_bytes: 16,
            ..limits()
        },
    );
    assert!(
        call(&deployment, "huge", b"")
            .await
            .unwrap_err()
            .to_string()
            .contains("output exceeds")
    );
    deployment.shutdown().await.unwrap();
    let huge_error = binary(
        r#"(module
        (import "extism:host/env" "alloc" (func $alloc (param i64) (result i64)))
        (import "extism:host/env" "error_set" (func $error (param i64)))
        (func (export "fail") (result i32)
            (call $error (call $alloc (i64.const 65536))) (i32.const 1)))"#,
    );
    // Direct vendor regression: the SDK itself, before Alia maps errors, is bounded.
    let error = tokio::task::spawn_blocking(move || {
        let mut plugin = extism::PluginBuilder::new(huge_error)
            .with_cache_disabled()
            .with_wasi(false)
            .build()
            .unwrap();
        plugin
            .call::<_, &[u8]>("fail", b"".as_slice())
            .unwrap_err()
            .to_string()
    })
    .await
    .unwrap();
    assert!(error.contains("4096-byte budget"));
    assert!(error.len() < 256);
}

#[tokio::test]
async fn package_lifecycle_pins_grants_and_invalidates_old_tool_references() {
    let root = tempfile::tempdir().unwrap();
    let store = PackageStore::open(root.path().to_path_buf()).unwrap();
    let mut manifest = PackageManifest {
        name: "probe".into(),
        version: "1".into(),
        abi_version: 1,
        limits: limits(),
        tools: vec![],
        expression: None,
    };
    let installed = store.install(manifest.clone(), echo()).await.unwrap();
    assert!(!installed.enabled);
    assert!(store.enable("probe", "wrong").await.is_err());
    let first = store
        .enable("probe", &installed.implementation_pin)
        .await
        .unwrap();
    assert_eq!(call(&first, "echo", b"old").await.unwrap(), b"old");
    store
        .revoke("probe", first.implementation_pin())
        .await
        .unwrap();
    let second = store
        .enable("probe", first.implementation_pin())
        .await
        .unwrap();
    assert!(!first.is_live());
    assert!(second.is_live());
    manifest.version = "2".into();
    let upgraded = store
        .upgrade(manifest, echo(), second.implementation_pin())
        .await
        .unwrap();
    assert!(!second.is_live());
    assert!(!upgraded.enabled);
    assert_ne!(upgraded.implementation_pin, installed.implementation_pin);
    assert!(
        store
            .enable("probe", &installed.implementation_pin)
            .await
            .is_err()
    );
    let current = store
        .enable("probe", &upgraded.implementation_pin)
        .await
        .unwrap();
    store
        .uninstall("probe", current.implementation_pin())
        .await
        .unwrap();
    assert!(!current.is_live());
    assert!(store.list().await.is_empty());
    assert!(
        PackageStore::open(root.path().to_path_buf())
            .unwrap()
            .list()
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn real_rust_pdk_exports_execute_under_the_same_runtime() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../examples/wasm-plugins/target/wasm32-unknown-unknown/release/alia_wasm_demo.wasm",
    );
    let module = std::fs::read(path).expect("build the Rust PDK example first: cd examples/wasm-plugins && cargo +1.96.0 build --release --locked");
    let deployment = enabled(module, limits());
    let input = serde_json::to_vec(&alia_plugin_api::ProcessorRequest {
        instance_id: "instance".into(),
        command_id: "command".into(),
        action: "echo".into(),
        payload: serde_json::json!({"hello":"Rust"}),
    })
    .unwrap();
    let output = call(&deployment, "alia_processor", &input).await.unwrap();
    let response: alia_plugin_api::ProcessorResponse = serde_json::from_slice(&output).unwrap();
    assert!(response.success);
    assert_eq!(response.result, serde_json::json!({"hello":"Rust"}));
    let output = call(&deployment, "alia_tool", br#"{"value":7}"#)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
        serde_json::json!({"value":7})
    );
    deployment.shutdown().await.unwrap();
}

#[tokio::test]
async fn initialization_and_export_loops_are_fuel_bounded() {
    for module in [
        "(module (func (export \"__wasm_call_ctors\") (loop $l (br $l))) (func (export \"run\") (result i32) (i32.const 0)))",
        "(module (func (export \"run\") (result i32) (loop $l (br $l)) (i32.const 0)))",
    ] {
        let deployment = enabled(binary(module), limits());
        assert!(call(&deployment, "run", b"").await.is_err());
        tokio::time::timeout(Duration::from_secs(10), deployment.shutdown())
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn failed_grant_persistence_still_closes_current_runtime() {
    let root = tempfile::tempdir().unwrap();
    let store = PackageStore::open(root.path().to_path_buf()).unwrap();
    let installed = store
        .install(
            PackageManifest {
                name: "probe".into(),
                version: "1".into(),
                abi_version: 1,
                limits: limits(),
                tools: vec![],
                expression: None,
            },
            echo(),
        )
        .await
        .unwrap();
    let deployment = store
        .enable("probe", &installed.implementation_pin)
        .await
        .unwrap();
    let package = root.path().join("probe");
    let moved = root.path().join(".probe-moved");
    std::fs::rename(&package, &moved).unwrap();
    let error = store
        .revoke("probe", deployment.implementation_pin())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("durability is unconfirmed"));
    assert!(!deployment.is_live());
    assert!(moved.join("grant").is_file());
    std::fs::rename(&moved, &package).unwrap();
    store
        .revoke("probe", deployment.implementation_pin())
        .await
        .unwrap();
    assert!(!package.join("grant").exists());
    let deployment = store
        .enable("probe", &installed.implementation_pin)
        .await
        .unwrap();
    let grant = root.path().join("probe/grant");
    std::fs::remove_file(&grant).unwrap();
    std::fs::create_dir(&grant).unwrap();
    let error = store
        .revoke("probe", deployment.implementation_pin())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("durability is unconfirmed"));
    assert!(!deployment.is_live());
    assert!(call(&deployment, "echo", b"must not run").await.is_err());
    std::fs::remove_dir(&grant).unwrap();
    store
        .uninstall("probe", deployment.implementation_pin())
        .await
        .unwrap();
}
