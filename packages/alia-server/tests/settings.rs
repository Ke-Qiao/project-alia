use std::path::Path;

use alia_server::config::ServerConfig;
use alia_server::settings::SettingsRegistry;

#[tokio::test]
async fn memory_backend_schema_matches_runtime_backends() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap();
    let settings = SettingsRegistry::new();
    settings.seed_from_config(&config).await;

    let entry = settings
        .get_entry("core.modules.memory")
        .await
        .expect("memory module setting should be registered");
    let schema = entry
        .schema
        .expect("memory module setting should expose a schema");

    assert_eq!(
        schema.options.as_deref(),
        Some(&["mem0".to_string(), "simple".to_string(), "noop".to_string()][..])
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
    ] {
        assert!(
            settings.get_entry(key).await.is_none(),
            "{key} should be a fixed core service, not a configurable slot"
        );
    }
}
