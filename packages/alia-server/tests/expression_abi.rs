use alia_server::expression::abi::ensure_supported_expression_plugin_abi;
use alia_server::expression::{EXPRESSION_PLUGIN_ABI_VERSION, ExpressionPluginManifest};

#[test]
fn manifest_defaults_are_safe_for_dynamic_plugins() {
    let manifest: ExpressionPluginManifest = toml::from_str(
        r#"
name = "demo"
command = ["demo-plugin"]
[capabilities]
appraisal_engine = true
"#,
    )
    .unwrap();

    assert!(manifest.enabled);
    assert_eq!(manifest.abi_version, EXPRESSION_PLUGIN_ABI_VERSION);
    assert!(manifest.cancel_on_timeout);
    assert!(manifest.sandbox.clear_env);
    assert!(manifest.capabilities.appraisal_engine);
}

#[test]
fn rejects_unsupported_plugin_abi_versions() {
    let error = ensure_supported_expression_plugin_abi(EXPRESSION_PLUGIN_ABI_VERSION + 1)
        .unwrap_err()
        .to_string();

    assert!(error.contains("unsupported expression plugin ABI version"));
}
