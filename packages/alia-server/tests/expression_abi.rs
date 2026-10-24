use alia_server::expression::EXPRESSION_PLUGIN_ABI_VERSION;
use alia_server::expression::abi::{WasmExpressionConfig, ensure_supported_expression_plugin_abi};

#[test]
fn wasm_expression_metadata_has_explicit_version_and_no_native_commands() {
    let config: WasmExpressionConfig = serde_json::from_value(serde_json::json!({
        "abi_version": 3, "capabilities":{"appraisal_engine":true}
    }))
    .unwrap();
    assert_eq!(config.abi_version, EXPRESSION_PLUGIN_ABI_VERSION);
    assert!(config.capabilities.appraisal_engine);
    assert!(
        serde_json::from_value::<WasmExpressionConfig>(serde_json::json!({
            "abi_version":3,"command":["python3","plugin.py"]
        }))
        .is_err()
    );
}

#[test]
fn rejects_replaced_json_process_abi_versions() {
    assert!(ensure_supported_expression_plugin_abi(1).is_err());
    assert!(ensure_supported_expression_plugin_abi(2).is_err());
    assert!(ensure_supported_expression_plugin_abi(EXPRESSION_PLUGIN_ABI_VERSION).is_ok());
}
