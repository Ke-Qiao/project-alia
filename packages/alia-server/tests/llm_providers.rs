use std::collections::BTreeMap;
use std::path::Path;

use alia_server::config::{
    LlmProfileKind, LlmProfileSection, LlmRequestOptionsSection, ServerConfig,
};
use alia_server::module::factory::{create_embedding, create_llm};

#[test]
fn creates_provider_neutral_llm_adapters() {
    let mut config = test_config();
    insert_llm_profile(&mut config, "openai_test", "openai", LlmProfileKind::Chat);
    insert_llm_profile(
        &mut config,
        "anthropic_test",
        "anthropic",
        LlmProfileKind::Chat,
    );
    insert_llm_profile(&mut config, "gemini_test", "gemini", LlmProfileKind::Chat);

    assert_eq!(create_llm(&config, "openai_test").unwrap().name(), "openai");
    assert_eq!(
        create_llm(&config, "anthropic_test").unwrap().name(),
        "anthropic"
    );
    assert_eq!(create_llm(&config, "gemini_test").unwrap().name(), "gemini");
}

#[test]
fn unknown_llm_provider_falls_back_to_noop() {
    let mut config = test_config();
    insert_llm_profile(&mut config, "unknown_test", "unknown", LlmProfileKind::Chat);

    assert_eq!(
        create_llm(&config, "unknown_test").unwrap().name(),
        "noop_llm"
    );
}

#[test]
fn unknown_embedding_provider_falls_back_to_noop() {
    let mut config = test_config();
    insert_llm_profile(
        &mut config,
        "unknown_embedding_test",
        "unknown",
        LlmProfileKind::Embedding,
    );

    assert_eq!(
        create_embedding(&config, "unknown_embedding_test")
            .unwrap()
            .name(),
        "noop_embedding"
    );
}

#[test]
fn missing_llm_profile_returns_error() {
    let config = test_config();

    assert!(create_llm(&config, "missing_profile").is_err());
}

fn test_config() -> ServerConfig {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap()
}

fn insert_llm_profile(
    config: &mut ServerConfig,
    profile_id: &str,
    provider: &str,
    kind: LlmProfileKind,
) {
    config.profiles.llm.insert(
        profile_id.to_string(),
        LlmProfileSection {
            provider: provider.to_string(),
            model: "test-model".to_string(),
            kind,
            dimensions: (kind == LlmProfileKind::Embedding).then_some(1536),
            request: LlmRequestOptionsSection::default(),
            provider_specific: BTreeMap::new(),
        },
    );
}
