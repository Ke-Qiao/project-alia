use std::path::Path;

use alia_server::config::ServerConfig;
use alia_server::settings::SettingsRegistry;

#[tokio::test]
async fn memory_policy_settings_are_registered_without_backend_slot() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap();
    let settings = SettingsRegistry::new();
    settings.seed_from_config(&config).await;

    assert!(settings.get_entry("core.modules.memory").await.is_none());
    assert!(
        settings
            .get_entry("core.policies.memory.capture_mode")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.memory.capture_timeout_sec")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.memory.capture_min_confidence")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.memory.sensitive_filter_enabled")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.memory.retrieval_layers")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.memory.max_injected_memory_chars")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.llm.roles.memory_capture")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.insights.memory_consolidation_prompt")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.insights.memory_extraction_prompt")
            .await
            .is_some()
    );
}

#[tokio::test]
async fn expression_policy_settings_are_registered() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap();
    let settings = SettingsRegistry::new();
    settings.seed_from_config(&config).await;

    assert!(
        settings
            .get_entry("core.policies.expression.enabled")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.expression.default_style")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.providers.gpt_sovits.max_speed_factor")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.providers.gpt_sovits.max_pitch_shift_semitones")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.affect.enabled")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.affect.baseline_valence")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.affect.baseline_trust")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.affect.session_appraisal_ttl_sec")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.affect.relationship_appraisal_ttl_sec")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.affect.min_appraisal_confidence")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.affect.relationship_persistence_enabled")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.affect.persist_relationship_evidence")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.regulation.enabled")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.regulation.max_negative_expression")
            .await
            .is_some()
    );
}

#[tokio::test]
async fn core_services_are_not_exposed_as_replaceable_module_slots() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap();
    let settings = SettingsRegistry::new();
    settings.seed_from_config(&config).await;

    for key in [
        "core.modules.cache",
        "core.modules.router",
        "core.modules.rule_engine",
        "core.modules.scheduler",
        "core.modules.tools",
        "core.modules.skills",
        "core.modules.persona",
        "core.modules.insights",
        "core.modules.memory",
    ] {
        assert!(
            settings.get_entry(key).await.is_none(),
            "{key} should be a fixed core service, not a configurable slot"
        );
    }
}
