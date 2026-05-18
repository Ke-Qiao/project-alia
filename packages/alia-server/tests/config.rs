use std::path::Path;

use alia_server::config::{ServerConfig, load_config_with_fragments};

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

    assert_eq!(config.modules.llm, "openai");
    assert_eq!(config.modules.local_llm, "ollama");
    assert_eq!(config.modules.remote_llm, "openai");
    assert_eq!(config.providers.openai.chat_model, "qwen2.5-7b-instruct");
    assert_eq!(
        config.providers.whisper_cpp.endpoint,
        "http://localhost:8080"
    );
    assert_eq!(
        config.providers.gpt_sovits.endpoint,
        "http://localhost:9880/tts"
    );
    assert_eq!(config.policies.conversation.max_context_messages, 20);
    assert_eq!(config.resources.skills_dir.as_deref(), Some("../skills"));
}
