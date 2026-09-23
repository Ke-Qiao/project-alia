use std::path::Path;

use alia_server::config::{
    LlmProfileKind, LlmProviderProfileRef, MemoryCaptureMode, ServerConfig,
    load_config_with_fragments,
};

#[test]
fn duplex_hold_budget_defaults_and_explicit_bounds_are_checked_during_deserialization() {
    use alia_server::config::PoliciesSection;
    assert_eq!(PoliciesSection::default().duplex.hold_budget_ms, 60_000);
    assert_eq!(
        toml::from_str::<PoliciesSection>("")
            .unwrap()
            .duplex
            .hold_budget_ms,
        60_000
    );
    assert_eq!(
        toml::from_str::<PoliciesSection>("[duplex]")
            .unwrap()
            .duplex
            .hold_budget_ms,
        60_000
    );
    for budget in [1, 3_000, 60_000, 120_000] {
        let value =
            toml::from_str::<PoliciesSection>(&format!("[duplex]\nhold_budget_ms = {budget}"))
                .unwrap();
        assert_eq!(value.duplex.hold_budget_ms, budget);
    }
    for invalid in ["0", "120001", "-1", "60000.5", "\"60000\""] {
        assert!(
            toml::from_str::<PoliciesSection>(&format!("[duplex]\nhold_budget_ms = {invalid}"))
                .is_err()
        );
    }
}

#[test]
fn loads_config_fragments_recursively_in_sorted_order() {
    let root = std::env::temp_dir().join(format!(
        "alia-config-fragments-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let config_path = root.join("server.toml");
    let fragment_dir = root.join("server.d");
    let nested_dir = fragment_dir.join("plugins");

    std::fs::create_dir_all(&nested_dir).unwrap();
    std::fs::write(
        &config_path,
        r#"
value = "base"

[providers.llm.openai]
type = "openai"
base_url = "https://base.example/v1"
"#,
    )
    .unwrap();
    std::fs::write(
        fragment_dir.join("20-second.toml"),
        r#"
value = "second"

[profiles.llm.minor]
provider = "gemini"
model = "gemini-2.5-flash"
"#,
    )
    .unwrap();
    std::fs::write(fragment_dir.join("10-first.toml"), "value = \"first\"\n").unwrap();
    std::fs::write(nested_dir.join("30-plugin.toml"), "value = \"plugin\"\n").unwrap();
    std::fs::write(fragment_dir.join("ignore.md"), "ignored = true\n").unwrap();

    let loaded = load_config_with_fragments(&config_path).unwrap();
    let loaded: toml::Value = loaded.parse().unwrap();
    assert_eq!(loaded["value"].as_str(), Some("plugin"));
    assert_eq!(
        loaded["providers"]["llm"]["openai"]["base_url"].as_str(),
        Some("https://base.example/v1")
    );
    assert_eq!(
        loaded["profiles"]["llm"]["minor"]["model"].as_str(),
        Some("gemini-2.5-flash")
    );
    assert!(loaded.get("ignored").is_none());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_legacy_top_level_llm_provider_sections() {
    let err = toml::from_str::<ServerConfig>(
        r#"
[server]
host = "0.0.0.0"
port = 3000
log_level = "info"

[database]
url = "postgres://alia:alia_dev@localhost/alia?sslmode=disable"
max_connections = 10
min_connections = 2

[auth]
jwt_secret = "change-me-in-production"
server_keypair_path = "server.key"
jwt_expiry_hours = 24
refresh_expiry_days = 30
argon2_memory_kib = 19456
argon2_iterations = 2
argon2_parallelism = 1

[modules]
stt = "whisper_cpp"
tts = "gpt_sovits"

[providers.openai]
api_key = ""
base_url = "https://api.openai.com/v1"
api_family = "chat_completions"
health_probe_enabled = false
"#,
    )
    .unwrap_err();

    assert!(
        err.to_string().contains("unknown field `openai`"),
        "unexpected error: {err}"
    );
}

#[test]
fn parses_split_example_config() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap();

    assert_eq!(config.modules.stt, "whisper_cpp");
    assert_eq!(config.modules.tts, "gpt_sovits");
    assert_eq!(config.llm.roles.minor, "minor");
    assert_eq!(config.llm.roles.major, "major");
    assert!(config.llm.roles.transcript_correction.is_none());
    assert!(config.llm.roles.memory_capture.is_none());
    assert_eq!(
        config.llm.roles.sentence_expression.as_deref(),
        Some("sentence_expression")
    );
    assert_eq!(config.llm.roles.embedding, "embedding");
    let minor_llm = config.profiles.llm_profile("minor").unwrap();
    assert_eq!(minor_llm.provider, "gemini");
    assert_eq!(minor_llm.model, "gemini-2.5-flash");
    assert_eq!(minor_llm.kind, LlmProfileKind::Chat);
    assert_eq!(minor_llm.request.temperature, 0.4);
    let major_llm = config.profiles.llm_profile("major").unwrap();
    assert_eq!(major_llm.provider, "local_openai");
    assert_eq!(major_llm.model, "qwen2.5-7b-instruct");
    assert_eq!(major_llm.request.max_output_tokens, 4096);
    let LlmProviderProfileRef::OpenAi(local_openai) =
        config.llm_provider_profile("local_openai").unwrap()
    else {
        panic!("local_openai should be OpenAI-compatible");
    };
    assert_eq!(local_openai.base_url, "http://127.0.0.1:1234/v1");
    let embedding = config.profiles.llm_profile("embedding").unwrap();
    assert_eq!(embedding.provider, "openai");
    assert_eq!(embedding.kind, LlmProfileKind::Embedding);
    assert_eq!(embedding.model, "text-embedding-3-small");
    assert_eq!(embedding.dimensions, Some(1536));
    let sentence_expression = config.profiles.llm_profile("sentence_expression").unwrap();
    assert_eq!(sentence_expression.kind, LlmProfileKind::Chat);
    assert!(sentence_expression.request.json_mode);
    assert_eq!(
        config.providers.whisper_cpp.endpoint,
        "http://localhost:8080"
    );
    assert_eq!(
        config.providers.gpt_sovits.endpoint,
        "http://localhost:9880/tts"
    );
    assert_eq!(config.providers.gpt_sovits.max_speed_factor, 1.15);
    assert_eq!(config.providers.gpt_sovits.max_pitch_shift_semitones, 2.0);
    let default_voice = config.profiles.default_voice_profile();
    assert_eq!(default_voice.language, "zh");
    assert!(default_voice.provider_specific.contains_key("speed_factor"));
    assert_eq!(config.policies.conversation.max_context_messages, 20);
    assert!(config.policies.affect.enabled);
    assert_eq!(config.policies.affect.baseline_arousal, 0.35);
    assert_eq!(config.policies.affect.baseline_trust, 0.55);
    assert_eq!(config.policies.affect.session_appraisal_ttl_sec, 900);
    assert_eq!(
        config.policies.affect.relationship_appraisal_ttl_sec,
        7_776_000
    );
    assert!(config.policies.affect.relationship_persistence_enabled);
    assert!(!config.policies.affect.persist_relationship_evidence);
    assert!(config.policies.affect.relationship_profile_enabled);
    assert_eq!(config.policies.affect.max_active_appraisal_events, 64);
    assert!(config.policies.regulation.enabled);
    assert_eq!(config.policies.regulation.max_negative_expression, 0.15);
    assert!(config.policies.transcript_correction.enabled);
    assert_eq!(
        config.policies.transcript_correction.provider,
        "default_llm"
    );
    assert_eq!(config.policies.transcript_correction.timeout_sec, 2);
    assert_eq!(
        config
            .policies
            .transcript_correction
            .min_confidence_to_replace,
        0.55
    );
    assert_eq!(
        config.policies.memory.capture_mode,
        MemoryCaptureMode::LlmSelected
    );
    assert_eq!(config.policies.memory.capture_timeout_sec, 3);
    assert_eq!(config.policies.memory.capture_min_confidence, 0.65);
    assert!(config.policies.memory.sensitive_filter_enabled);
    assert_eq!(
        config.policies.memory.retrieval_layers,
        vec![
            "core".to_string(),
            "preference".to_string(),
            "relationship".to_string()
        ]
    );
    assert_eq!(config.policies.memory.max_injected_memory_chars, 1200);
    assert!(
        config
            .policies
            .insights
            .memory_consolidation_prompt
            .contains("consolidate a user's memory store")
    );
    assert!(
        config
            .policies
            .insights
            .memory_extraction_prompt
            .contains("durable user memories")
    );
    assert!(config.policies.expression.enabled);
    assert_eq!(config.policies.expression.default_style, "gentle");
    assert!(config.policies.expression.sentence_llm.enabled);
    assert_eq!(config.policies.expression.sentence_llm.timeout_ms, 250);
    assert_eq!(config.policies.expression.sentence_llm.min_confidence, 0.65);
    assert!(config.policies.expression.sentence_llm.cache_enabled);
    assert!(config.profiles.expression_styles.contains_key("gentle"));
    assert_eq!(
        config
            .profiles
            .expression_styles
            .get("cheerful")
            .and_then(|style| style.voice_profile.as_deref()),
        Some("default")
    );
    assert!(config.policies.tools.indexed_search_enabled);
    assert_eq!(config.policies.tools.indexed_search_min_similarity, 0.3);
    assert!(
        config
            .policies
            .tools
            .indexed_search_refresh_missing_embeddings
    );
    assert_eq!(config.resources.skills_dir.as_deref(), Some("../skills"));
    assert!(
        config
            .resources
            .cache_policy
            .dynamic_query_keywords
            .contains(&"latest".to_string())
    );
    assert!(config.resources.appraisal_lexicons.contains_key("zh"));
    assert!(config.resources.appraisal_lexicons.contains_key("en"));
    assert!(
        config.resources.appraisal_lexicons["en"]
            .support_need
            .contains(&"anxious".to_string())
    );
}
