use std::collections::BTreeMap;
use std::path::Path;

use alia_server::bootstrap::factory::{create_embedding, create_llm};
use alia_server::config::{
    LlmProfileKind, LlmProfileSection, LlmProviderProfileRef, LlmProviderProfileSection,
    LlmRequestOptionsSection, OpenAiApiFamily, OpenAiProviderSection, ServerConfig,
};

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
fn unknown_llm_provider_is_rejected_and_noop_requires_explicit_selection() {
    let mut config = test_config();
    insert_llm_profile(&mut config, "unknown_test", "unknown", LlmProfileKind::Chat);

    let error = create_llm(&config, "unknown_test")
        .err()
        .expect("unknown providers must fail configuration validation");
    assert!(error.to_string().contains("not configured"));
    assert_eq!(create_llm(&config, "noop").unwrap().name(), "noop_llm");
}

#[test]
fn unknown_embedding_provider_is_rejected_and_noop_requires_explicit_selection() {
    let mut config = test_config();
    insert_llm_profile(
        &mut config,
        "unknown_embedding_test",
        "unknown",
        LlmProfileKind::Embedding,
    );

    let error = create_embedding(&config, "unknown_embedding_test")
        .err()
        .expect("unknown providers must fail configuration validation");
    assert!(error.to_string().contains("not configured"));
    assert_eq!(
        create_embedding(&config, "noop").unwrap().name(),
        "noop_embedding"
    );
}

#[test]
fn missing_llm_profile_returns_error() {
    let config = test_config();

    assert!(create_llm(&config, "missing_profile").is_err());
}

#[test]
fn llm_profiles_can_reference_named_provider_profiles() {
    let mut config = test_config();
    config.providers.llm.insert(
        "openrouter".to_string(),
        LlmProviderProfileSection::OpenAi(OpenAiProviderSection {
            api_key: "test-openrouter-key".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            api_family: OpenAiApiFamily::ChatCompletions,
            health_probe_enabled: false,
        }),
    );
    insert_llm_profile(
        &mut config,
        "openrouter_major",
        "openrouter",
        LlmProfileKind::Chat,
    );

    assert_eq!(
        create_llm(&config, "openrouter_major").unwrap().name(),
        "openai"
    );
    let LlmProviderProfileRef::OpenAi(provider) =
        config.llm_provider_profile("openrouter").unwrap()
    else {
        panic!("openrouter should resolve to an OpenAI-compatible provider profile");
    };
    assert_eq!(provider.base_url, "https://openrouter.ai/api/v1");
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

#[test]
fn decision_profiles_cannot_be_used_as_chat_or_embedding_and_legacy_needs_no_key() {
    use alia_server::bootstrap::factory::create_router_decision;
    use alia_server::providers::llm::LlmService;
    let mut config = test_config();
    assert!(create_router_decision(&config, "legacy").unwrap().is_none());
    assert!(create_llm(&config, "jev").is_err());
    assert!(create_embedding(&config, "jev").is_err());
    assert!(create_router_decision(&config, "minor").is_err());
    let provider = config.llm_provider_profile("typesafe").unwrap();
    assert!(LlmService::from_profile("jev", &config.profiles.llm["jev"], provider).is_err());
    let mut chat = config.profiles.llm["minor"].clone();
    chat.provider = "typesafe".into();
    config.profiles.llm.insert("wrong_chat".into(), chat);
    assert!(create_llm(&config, "wrong_chat").is_err());
    let mut embedding = config.profiles.llm["embedding"].clone();
    embedding.provider = "typesafe".into();
    config
        .profiles
        .llm
        .insert("wrong_embedding".into(), embedding);
    assert!(create_embedding(&config, "wrong_embedding").is_err());
}
