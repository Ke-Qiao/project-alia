use std::path::Path;

use alia_server::config::{MemoryCaptureMode, ServerConfig, load_config_with_fragments};

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

[providers.openai]
chat_model = "base"
"#,
    )
    .unwrap();
    std::fs::write(
        fragment_dir.join("20-second.toml"),
        r#"
value = "second"

[providers.ollama]
chat_model = "qwen2.5:7b"
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
        loaded["providers"]["openai"]["chat_model"].as_str(),
        Some("base")
    );
    assert_eq!(
        loaded["providers"]["ollama"]["chat_model"].as_str(),
        Some("qwen2.5:7b")
    );
    assert!(loaded.get("ignored").is_none());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn parses_split_example_config() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap();

    assert_eq!(config.modules.minor_llm, "ollama");
    assert_eq!(config.modules.major_llm, "openai");
    assert!(config.modules.transcript_correction_llm.is_none());
    assert!(config.modules.memory_capture_llm.is_none());
    assert_eq!(config.modules.stt, "whisper_cpp");
    assert_eq!(config.modules.tts, "gpt_sovits");
    assert_eq!(config.modules.embedding, "ollama");
    assert_eq!(config.modules.memory, "mem0");
    assert_eq!(config.providers.openai.chat_model, "qwen2.5-7b-instruct");
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
    assert!(!config.providers.mem0.conversation_infer);
    assert!(config.policies.expression.enabled);
    assert_eq!(config.policies.expression.default_style, "gentle");
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
