use std::path::Path;

use alia_server::config::ServerConfig;
use alia_server::settings::SettingsRegistry;
use alia_server::settings::registry::SettingSchema;
use serde_json::json;

struct SettingsDirectory(std::path::PathBuf);

impl SettingsDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("alia-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for SettingsDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn private_schema() -> SettingSchema {
    SettingSchema {
        label: "Isolated fixture credential".into(),
        description: None,
        value_type: "secret".into(),
        default: None,
        options: None,
        requires_restart: false,
        secret: true,
    }
}

#[tokio::test]
async fn voice_interruption_setting_is_exported_as_restart_required_and_persists() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap();
    let settings = SettingsRegistry::new();
    settings.seed_from_config(&config).await;
    let key = "core.policies.duplex.barge_in_enabled";
    let entry = settings.get_entry(key).await.unwrap();
    assert_eq!(entry.value, json!(false));
    let schema = entry.schema.unwrap();
    assert_eq!(schema.value_type, "bool");
    assert!(schema.requires_restart && !schema.secret);
    let directory = SettingsDirectory::new();
    let path = directory.0.join("settings.toml");
    settings.set_persist_path(path.clone()).await;
    settings.set(key, json!(true)).await;
    settings.save().await.unwrap();
    let reloaded = SettingsRegistry::new();
    reloaded.seed_from_config(&config).await;
    reloaded.load_persisted(&path).await.unwrap();
    assert_eq!(reloaded.get(key).await, Some(json!(true)));
    assert!(
        reloaded
            .get_entry(key)
            .await
            .unwrap()
            .schema
            .unwrap()
            .requires_restart
    );
}

#[tokio::test]
async fn persisted_values_exclude_sql_policy_and_null_and_preserve_registered_schema() {
    let directory = SettingsDirectory::new();
    let path = directory.0.join("nested/settings.toml");
    let settings = SettingsRegistry::new();
    settings
        .register(
            "plugin.fixture.credential",
            json!("fixture-only-secret"),
            private_schema(),
        )
        .await;
    settings
        .set("plugin.fixture.targets", json!(["one", "two"]))
        .await;
    settings
        .set("plugin.fixture.unset", serde_json::Value::Null)
        .await;
    settings
        .set("core.policies.memory.enabled", json!(false))
        .await;
    settings
        .set("core.policies.affect.enabled", json!(true))
        .await;
    settings.set_persist_path(path.clone()).await;
    settings.save().await.unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let values: std::collections::BTreeMap<String, serde_json::Value> =
        toml::from_str(&text).unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!(values["plugin.fixture.credential"], "fixture-only-secret");
    assert_eq!(values["plugin.fixture.targets"], json!(["one", "two"]));
    assert!(
        !text.contains("Isolated fixture credential"),
        "schema metadata must not be serialized as a setting"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let loaded = SettingsRegistry::new();
    loaded
        .register(
            "plugin.fixture.credential",
            json!("deployment-default"),
            private_schema(),
        )
        .await;
    loaded.load_persisted(&path).await.unwrap();
    let entry = loaded.get_entry("plugin.fixture.credential").await.unwrap();
    assert_eq!(entry.value, "fixture-only-secret");
    let schema = entry.schema.unwrap();
    assert_eq!(schema.label, "Isolated fixture credential");
    assert!(schema.secret);
    assert_eq!(
        loaded.get("plugin.fixture.targets").await,
        Some(json!(["one", "two"]))
    );
    assert!(loaded.get("core.policies.memory.enabled").await.is_none());
    assert!(loaded.get("plugin.fixture.unset").await.is_none());
}

#[tokio::test]
async fn invalid_persisted_file_does_not_partially_replace_values_or_the_save_target() {
    let directory = SettingsDirectory::new();
    let path = directory.0.join("settings.toml");
    let invalid = directory.0.join("invalid.toml");
    let settings = SettingsRegistry::new();
    settings.set("plugin.fixture.level", json!(1)).await;
    settings.set_persist_path(path.clone()).await;
    settings.save().await.unwrap();
    let invalid_text = "\"plugin.fixture.level\" = 999\n[broken\n";
    std::fs::write(&invalid, invalid_text).unwrap();
    assert!(settings.load_persisted(&invalid).await.is_err());
    assert_eq!(settings.get("plugin.fixture.level").await, Some(json!(1)));
    settings.set("plugin.fixture.level", json!(2)).await;
    settings.save().await.unwrap();
    let fresh = SettingsRegistry::new();
    fresh.load_persisted(&path).await.unwrap();
    assert_eq!(fresh.get("plugin.fixture.level").await, Some(json!(2)));
    assert_eq!(std::fs::read_to_string(&invalid).unwrap(), invalid_text);
}

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
            .get_entry("core.llm.roles.sentence_expression")
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
            .get_entry("core.policies.expression.sentence_llm.enabled")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.expression.sentence_llm.timeout_ms")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.expression.sentence_llm.min_confidence")
            .await
            .is_some()
    );
    assert!(
        settings
            .get_entry("core.policies.expression.sentence_llm.cache_enabled")
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

#[tokio::test]
async fn decision_routing_identity_restarts_and_legacy_restore_survives_persistence() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut config = ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap();
    let directory = SettingsDirectory::new();
    let path = directory.0.join("settings.toml");
    let settings = SettingsRegistry::new();
    settings.seed_from_config(&config).await;
    settings.set_persist_path(path.clone()).await;
    for key in [
        "core.llm.roles.routing",
        "core.profiles.llm.jev.provider",
        "core.profiles.llm.jev.kind",
        "core.profiles.llm.jev.model",
        "core.profiles.llm.jev.provider_specific.expected_model",
        "core.providers.llm.typesafe.type",
        "core.providers.llm.typesafe.base_url",
        "core.providers.llm.typesafe.api_key",
        "core.providers.llm.typesafe.api_key_env",
    ] {
        let entry = settings.get_entry(key).await.unwrap();
        assert!(entry.schema.unwrap().requires_restart, "{key}");
    }
    assert!(
        settings
            .get_entry("core.providers.llm.typesafe.api_key")
            .await
            .unwrap()
            .schema
            .unwrap()
            .secret
    );
    assert!(
        settings
            .get_entry("core.profiles.llm.jev.temperature")
            .await
            .is_none()
    );
    settings.set("core.llm.roles.routing", json!("jev")).await;
    settings.save().await.unwrap();
    let reloaded = SettingsRegistry::new();
    reloaded.seed_from_config(&config).await;
    reloaded.load_persisted(&path).await.unwrap();
    let values = reloaded
        .all()
        .await
        .into_iter()
        .map(|(key, entry)| (key, entry.value))
        .collect();
    config.apply_routing_settings(&values).unwrap();
    assert_eq!(config.llm.roles.routing, "jev");
    // A non-null disabled value must override even a base config that now enables Jev.
    reloaded
        .set("core.llm.roles.routing", json!("legacy"))
        .await;
    reloaded.save().await.unwrap();
    let restored = SettingsRegistry::new();
    restored.seed_from_config(&config).await;
    restored.load_persisted(&path).await.unwrap();
    let values = restored
        .all()
        .await
        .into_iter()
        .map(|(key, entry)| (key, entry.value))
        .collect();
    config.apply_routing_settings(&values).unwrap();
    assert_eq!(config.llm.roles.routing, "legacy");
    assert!(std::fs::read_to_string(path).unwrap().contains("legacy"));
}
