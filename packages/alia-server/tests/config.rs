use std::path::Path;

use alia_server::config::{
    LlmProfileKind, LlmProviderProfileRef, MemoryCaptureMode, ServerConfig,
    load_config_with_fragments,
};

#[test]
fn retired_expression_process_configuration_is_rejected() {
    let error = toml::from_str::<alia_server::config::ResourcesSection>(
        "external_expression_plugins_dir = 'plugins'",
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown field `external_expression_plugins_dir`")
    );
}

#[test]
fn approved_animation_catalog_is_local_empty_by_default_and_validated_on_load() {
    use alia_core::embodiment::motion::{MotionAnimationAsset, motion_animation_sha256};
    use alia_server::config::ChannelConfig;
    struct TemporaryConfig(std::path::PathBuf);
    impl Drop for TemporaryConfig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = TemporaryConfig(
        std::env::temp_dir().join(format!("alia-approved-assets-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(&root.0).unwrap();
    let path = root.0.join("server.toml");
    let mut value: toml::Value =
        toml::from_str(include_str!("../../../config/server.example.toml")).unwrap();
    std::fs::write(&path, toml::to_string(&value).unwrap()).unwrap();
    let config = ServerConfig::load(&path).unwrap();
    assert!(
        matches!(&config.channels[0], ChannelConfig::Embedded { approved_animations, .. } if approved_animations.is_empty())
    );
    let content = b"frames\n0 0 0\n";
    let asset = MotionAnimationAsset {
        file: "reviewed.anim".into(),
        version: 1,
        sha256: motion_animation_sha256(content),
        bytes: content.len() as u32,
    };
    let approved = toml::Value::try_from(&asset).unwrap();
    value["channels"][0]["approved_animations"] = toml::Value::Array(vec![approved.clone()]);
    std::fs::write(&path, toml::to_string(&value).unwrap()).unwrap();
    let config = ServerConfig::load(&path).unwrap();
    assert!(
        matches!(&config.channels[0], ChannelConfig::Embedded { approved_animations, .. } if approved_animations == &[asset])
    );
    value["channels"][0]["approved_animations"] =
        toml::Value::Array(vec![approved.clone(), approved]);
    std::fs::write(&path, toml::to_string(&value).unwrap()).unwrap();
    assert!(ServerConfig::load(&path).is_err());
    value["channels"][0]["approved_animations"] =
        toml::Value::Array(vec![toml::Value::Table(toml::Table::from_iter([
            ("file".into(), toml::Value::String("reviewed.anim".into())),
            ("version".into(), toml::Value::Integer(0)),
            ("sha256".into(), toml::Value::String("a".repeat(64))),
            ("bytes".into(), toml::Value::Integer(10)),
        ]))]);
    std::fs::write(&path, toml::to_string(&value).unwrap()).unwrap();
    assert!(ServerConfig::load(&path).is_err());
}

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
fn duplex_voice_interruption_is_disabled_unless_explicitly_enabled() {
    use alia_server::config::PoliciesSection;
    assert!(!PoliciesSection::default().duplex.barge_in_enabled);
    for content in ["", "[duplex]", "[duplex]\nbarge_in_enabled = false"] {
        assert!(
            !toml::from_str::<PoliciesSection>(content)
                .unwrap()
                .duplex
                .barge_in_enabled
        );
    }
    assert!(
        toml::from_str::<PoliciesSection>("[duplex]\nbarge_in_enabled = true")
            .unwrap()
            .duplex
            .barge_in_enabled
    );
    for invalid in ["0", "1", "\"false\""] {
        assert!(
            toml::from_str::<PoliciesSection>(&format!("[duplex]\nbarge_in_enabled = {invalid}"))
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

fn decision_config() -> ServerConfig {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    ServerConfig::load(&workspace.join("config/server.example.toml")).unwrap()
}

#[test]
fn decision_profile_is_optional_and_rejects_wrong_protocol_or_mutable_identity() {
    use alia_server::config::LlmRolesSection;
    assert_eq!(
        toml::from_str::<LlmRolesSection>("").unwrap().routing,
        "legacy"
    );
    let mut config = decision_config();
    assert_eq!(config.llm.roles.routing, "legacy");
    config.validate_routing_profile().unwrap();
    config.llm.roles.routing = "jev".into();
    config.validate_routing_profile().unwrap();
    for role in ["", "missing", "minor", "embedding", "noop"] {
        let mut bad = config.clone();
        bad.llm.roles.routing = role.into();
        assert!(bad.validate_routing_profile().is_err(), "role {role}");
    }
    for model in [
        "jev",
        "jev-latest",
        "jev-preview",
        "jev-1.14",
        "jev-1.14.0-preview",
        "typesafe/jev-router",
        "typesafe/jev-1.14",
    ] {
        let mut bad = config.clone();
        bad.profiles.llm.get_mut("jev").unwrap().model = model.into();
        assert!(bad.validate_routing_profile().is_err(), "model {model}");
    }
    let mut bad = config.clone();
    bad.profiles.llm.get_mut("jev").unwrap().provider = "openai".into();
    assert!(bad.validate_routing_profile().is_err());
    let mut bad = config.clone();
    bad.profiles.llm.get_mut("jev").unwrap().kind = LlmProfileKind::Chat;
    assert!(bad.validate_routing_profile().is_err());
}

#[test]
fn decision_openrouter_request_and_returned_identity_are_pinned_separately() {
    let mut config = decision_config();
    config.llm.roles.routing = "jev".into();
    let profile = config.profiles.llm.get_mut("jev").unwrap();
    profile.model = "typesafe/jev-1.14".into();
    profile.provider_specific.insert(
        "expected_model".into(),
        serde_json::json!("typesafe/jev-1.14-20261014"),
    );
    config.validate_routing_profile().unwrap();
    for expected in [
        "typesafe/jev-1.14",
        "typesafe/jev-1.14-latest",
        "typesafe/jev-1.14-20261399",
        "typesafe/jev-1.13-20261014",
        "jev-1.14.0",
    ] {
        let mut bad = config.clone();
        bad.profiles
            .llm
            .get_mut("jev")
            .unwrap()
            .provider_specific
            .insert("expected_model".into(), serde_json::json!(expected));
        assert!(
            bad.validate_routing_profile().is_err(),
            "expected {expected}"
        );
    }
    config.profiles.llm.get_mut("jev").unwrap().model = "typesafe/jev-1.14-20261014".into();
    config
        .profiles
        .llm
        .get_mut("jev")
        .unwrap()
        .provider_specific
        .clear();
    config.validate_routing_profile().unwrap();
}

#[test]
fn decision_settings_validate_atomically_and_do_not_accept_chat_parameters() {
    use serde_json::json;
    use std::collections::BTreeMap;
    let mut config = decision_config();
    for (key, value) in [
        ("core.llm.roles.routing", json!(false)),
        ("core.llm.roles.routing", serde_json::Value::Null),
        ("core.llm.roles.routing", json!("missing")),
        ("core.profiles.llm.jev.model", json!("jev-latest")),
        ("core.profiles.llm.jev.kind", json!("chat")),
        ("core.profiles.llm.jev.provider", json!("openai")),
        ("core.profiles.llm.jev.temperature", json!(0.5)),
        (
            "core.profiles.llm.jev.provider_specific.expected_model",
            json!(false),
        ),
        ("core.providers.llm.typesafe.type", json!("openai")),
        (
            "core.providers.llm.typesafe.base_url",
            json!("http://example.com/decisions"),
        ),
        (
            "core.providers.llm.typesafe.base_url",
            json!("https://user:secret@example.com/decisions"),
        ),
        ("core.providers.llm.typesafe.api_key_env", json!("BAD=ENV")),
    ] {
        let old = config.clone();
        let changes = BTreeMap::from([
            ("core.llm.roles.routing".into(), json!("jev")),
            (key.into(), value),
        ]);
        assert!(
            config.apply_routing_settings(&changes).is_err(),
            "key {key}"
        );
        assert_eq!(config.llm.roles.routing, old.llm.roles.routing);
        assert_eq!(
            config.profiles.llm["jev"].model,
            old.profiles.llm["jev"].model
        );
    }
    config
        .apply_routing_settings(&BTreeMap::from([
            ("core.llm.roles.routing".into(), json!("jev")),
            (
                "core.providers.llm.typesafe.base_url".into(),
                json!("http://127.0.0.1:12345/v1/systemone"),
            ),
        ]))
        .unwrap();
    assert_eq!(config.llm.roles.routing, "jev");
}

#[test]
fn decision_credentials_use_only_the_selected_environment_and_preserve_explicit_keys() {
    const CHILD: &str = "ALIA_TEST_DECISION_CREDENTIAL_CHILD";
    const FILE: &str = "ALIA_TEST_DECISION_SETTINGS_FILE";
    if std::env::var_os(CHILD).is_none() {
        struct Directory(std::path::PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root = Directory(
            std::env::temp_dir().join(format!("alia-decision-env-{}", uuid::Uuid::new_v4())),
        );
        std::fs::create_dir(&root.0).unwrap();
        for phase in ["initial", "rotated"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child.args(["--exact", "decision_credentials_use_only_the_selected_environment_and_preserve_explicit_keys"])
                .env(CHILD, phase)
                .env(FILE, root.0.join("settings.toml"))
                .env("TYPESAFE_API_KEY", "isolated-typesafe-key")
                .env("ALIA_TEST_OTHER_DECISION_KEY", if phase == "initial" { "isolated-other-key" } else { "isolated-other-key-rotated" });
            for key in [
                "OPENAI_API_KEY",
                "ANTHROPIC_API_KEY",
                "GEMINI_API_KEY",
                "DATABASE_URL",
                "ALIA_JWT_SECRET",
            ] {
                child.env_remove(key);
            }
            assert!(child.status().unwrap().success());
        }
        return;
    }
    use alia_server::config::{LlmProviderProfileSection, TypeSafeProviderSection};
    use alia_server::settings::SettingsRegistry;
    use serde_json::json;
    let mut config = decision_config();
    let LlmProviderProfileSection::TypeSafe(provider) = &config.providers.llm["typesafe"] else {
        panic!("missing provider")
    };
    assert!(
        provider.api_key.is_empty(),
        "config must retain only explicitly configured credentials"
    );
    assert_eq!(provider.resolved_api_key(), "isolated-typesafe-key");
    let explicit = TypeSafeProviderSection {
        api_key: "isolated-explicit-key".into(),
        ..Default::default()
    };
    assert_eq!(explicit.resolved_api_key(), "isolated-explicit-key");
    let path = std::path::PathBuf::from(std::env::var_os(FILE).unwrap());
    let initial = std::env::var(CHILD).unwrap() == "initial";
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let settings = SettingsRegistry::new();
        settings.seed_from_config(&config).await;
        settings.set_persist_path(path.clone()).await;
        if initial {
            // Change source A to B without copying A into the desired api_key field.
            settings
                .set(
                    "core.providers.llm.typesafe.api_key_env",
                    json!("ALIA_TEST_OTHER_DECISION_KEY"),
                )
                .await;
        } else {
            settings.load_persisted(&path).await.unwrap();
        }
        let values = settings
            .all()
            .await
            .into_iter()
            .map(|(key, entry)| (key, entry.value))
            .collect();
        config.apply_routing_settings(&values).unwrap();
        let LlmProviderProfileSection::TypeSafe(provider) = &config.providers.llm["typesafe"]
        else {
            panic!("missing provider")
        };
        assert!(provider.api_key.is_empty());
        assert_eq!(provider.api_key_env, "ALIA_TEST_OTHER_DECISION_KEY");
        assert_eq!(
            provider.resolved_api_key(),
            if initial {
                "isolated-other-key"
            } else {
                "isolated-other-key-rotated"
            }
        );
        settings.save().await.unwrap();
    });
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(!saved.contains("isolated-typesafe-key"));
    assert!(!saved.contains("isolated-other-key"));
}
