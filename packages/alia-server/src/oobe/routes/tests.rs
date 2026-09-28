use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::config::{LlmProfileKind, ServerConfig};

struct Fixture {
    directory: PathBuf,
    url: String,
    client: reqwest::Client,
    done: watch::Receiver<Option<SetupResult>>,
    stop: CancellationToken,
    serving: JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("alia-oobe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let (done_tx, done) = watch::channel(None);
        let app = routes().with_state(OobeState {
            done_tx: Arc::new(done_tx),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let serving = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            directory,
            url,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            done,
            stop,
            serving,
        }
    }

    async fn template(&self) -> String {
        self.client
            .post(format!("{}/api/v1/setup/generate-config", self.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["template"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn save(&self, path: &Path, config: &str) -> Value {
        self.client
            .post(format!("{}/api/v1/setup/save-config", self.url))
            .json(&json!({"path":path,"config":config}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    fn assert_ready(&self, target: &Path) {
        let done = self.done.borrow();
        let Some(SetupResult::ConfigReady(path)) = &*done else {
            panic!("missing ConfigReady")
        };
        assert_eq!(path, target);
    }

    fn assert_no_temporary_files(&self) {
        assert!(std::fs::read_dir(&self.directory).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".alia-setup-")
        }));
    }

    async fn close(mut self) {
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(3), &mut self.serving)
            .await
            .unwrap()
            .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.cancel();
        self.serving.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn assert_example_roles(config: &ServerConfig) {
    config.validate_llm_roles().unwrap();
    let major = config
        .profiles
        .llm_profile(&config.llm.roles.major)
        .unwrap();
    assert_eq!(major.provider, "local_openai");
    assert_eq!(major.model, "qwen2.5-7b-instruct");
    assert_eq!(
        config.llm.roles.sentence_expression.as_deref(),
        Some("sentence_expression")
    );
    for id in [
        "minor",
        "major",
        "transcript_correction",
        "memory_capture",
        "sentence_expression",
        "embedding",
    ] {
        let profile = config.profiles.llm_profile(id).unwrap();
        config.llm_provider_profile(&profile.provider).unwrap();
        assert_eq!(
            profile.kind,
            if id == "embedding" {
                LlmProfileKind::Embedding
            } else {
                LlmProfileKind::Chat
            }
        );
    }
}

#[tokio::test]
async fn generated_http_template_loads_all_example_roles_without_external_fragments() {
    let f = Fixture::new().await;
    let template = f.template().await;
    let target = f.directory.join("nested/server.toml");
    let result = f.save(&target, &template).await;
    assert_eq!(result["success"], true, "{result}");
    f.assert_ready(&target);
    assert!(!target.with_extension("d").exists());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), template);
    let config = ServerConfig::load(&target).unwrap();
    assert_example_roles(&config);
    assert_eq!(
        config.config_dir,
        target.parent().unwrap().canonicalize().unwrap()
    );
    f.close().await;
}

#[tokio::test]
async fn missing_role_profiles_leave_original_config_and_setup_state_unchanged() {
    let f = Fixture::new().await;
    let original = f.template().await;
    let target = f.directory.join("server.toml");
    std::fs::write(&target, &original).unwrap();
    for role in [
        "minor",
        "major",
        "default",
        "transcript_correction",
        "memory_capture",
        "sentence_expression",
        "embedding",
    ] {
        let mut config: toml::Value = toml::from_str(&original).unwrap();
        config["llm"]["roles"]
            .as_table_mut()
            .unwrap()
            .insert(role.into(), "missing-profile".into());
        let result = f.save(&target, &toml::to_string(&config).unwrap()).await;
        assert_eq!(result["success"], false, "{role}: {result}");
        let error = result["error"].as_str().unwrap();
        assert!(
            error.contains(role) && error.contains("missing-profile"),
            "{error}"
        );
        assert!(f.done.borrow().is_none());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), original);
        f.assert_no_temporary_files();
    }
    assert_example_roles(&ServerConfig::load(&target).unwrap());
    f.close().await;
}

#[tokio::test]
async fn wrong_role_kind_or_provider_is_rejected_before_publication() {
    let f = Fixture::new().await;
    let template = f.template().await;
    let target = f.directory.join("server.toml");
    for defect in [
        "chat_kind",
        "embedding_kind",
        "missing_provider",
        "unsupported_embedding_provider",
    ] {
        let mut config: toml::Value = toml::from_str(&template).unwrap();
        match defect {
            "chat_kind" => config["llm"]["roles"]["major"] = "embedding".into(),
            "embedding_kind" => config["llm"]["roles"]["embedding"] = "major".into(),
            "missing_provider" => {
                config["profiles"]["llm"]["major"]["provider"] = "missing-provider".into()
            }
            "unsupported_embedding_provider" => {
                config["profiles"]["llm"]["embedding"]["provider"] = "gemini".into()
            }
            _ => unreachable!(),
        }
        let result = f.save(&target, &toml::to_string(&config).unwrap()).await;
        assert_eq!(result["success"], false, "{defect}: {result}");
        assert!(result["error"].as_str().unwrap().contains("LLM role"));
        assert!(f.done.borrow().is_none());
        assert!(!target.exists());
    }
    f.close().await;
}

#[tokio::test]
async fn target_fragments_are_validated_and_preserved_with_the_proposed_base() {
    let f = Fixture::new().await;
    let template = f.template().await;
    let target = f.directory.join("server.toml");
    std::fs::write(&target, &template).unwrap();
    let fragments = target.with_extension("d");
    std::fs::create_dir(&fragments).unwrap();
    let overlay = fragments.join("override.toml");
    let invalid = "[llm.roles]\nmajor = 'missing-fragment-profile'\n";
    std::fs::write(&overlay, invalid).unwrap();
    let result = f.save(&target, &template).await;
    assert_eq!(result["success"], false, "{result}");
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("missing-fragment-profile")
    );
    assert!(f.done.borrow().is_none());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), template);
    assert_eq!(std::fs::read_to_string(&overlay).unwrap(), invalid);

    let valid = "[profiles.llm.major]\nmodel = 'operator-selected-model'\n";
    std::fs::write(&overlay, valid).unwrap();
    let result = f.save(&target, &template).await;
    assert_eq!(result["success"], true, "{result}");
    f.assert_ready(&target);
    let config = ServerConfig::load(&target).unwrap();
    config.validate_llm_roles().unwrap();
    assert_eq!(
        config.profiles.llm_profile("major").unwrap().model,
        "operator-selected-model"
    );
    assert_eq!(std::fs::read_to_string(&overlay).unwrap(), valid);
    f.close().await;
}

#[tokio::test]
async fn publication_io_failure_keeps_original_target_and_emits_no_ready() {
    let f = Fixture::new().await;
    let template = f.template().await;
    let target = f.directory.join("existing-directory.toml");
    std::fs::create_dir(&target).unwrap();
    let original = target.join("original");
    std::fs::write(&original, "retain original directory").unwrap();
    let result = f.save(&target, &template).await;
    assert_eq!(result["success"], false, "{result}");
    assert!(f.done.borrow().is_none());
    assert_eq!(
        std::fs::read_to_string(original).unwrap(),
        "retain original directory"
    );
    f.assert_no_temporary_files();
    f.close().await;
}

#[tokio::test]
async fn malformed_config_does_not_create_target_or_signal_ready() {
    let f = Fixture::new().await;
    let target = f.directory.join("new/server.toml");
    let result = f.save(&target, "[broken").await;
    assert_eq!(result["success"], false, "{result}");
    assert!(f.done.borrow().is_none());
    assert!(!target.parent().unwrap().exists());
    f.close().await;
}

#[tokio::test]
async fn noop_and_absent_optional_roles_do_not_validate_unused_profiles() {
    let f = Fixture::new().await;
    let mut config: toml::Value = toml::from_str(&f.template().await).unwrap();
    let roles = config["llm"]["roles"].as_table_mut().unwrap();
    for role in ["minor", "major", "default", "embedding"] {
        roles.insert(role.into(), "noop".into());
    }
    for role in [
        "transcript_correction",
        "memory_capture",
        "sentence_expression",
    ] {
        roles.remove(role);
    }
    config["profiles"]["llm"]["major"]["provider"] = "unused-missing-provider".into();
    let target = f.directory.join("server.toml");
    let result = f.save(&target, &toml::to_string(&config).unwrap()).await;
    assert_eq!(result["success"], true, "{result}");
    f.assert_ready(&target);
    ServerConfig::load(&target)
        .unwrap()
        .validate_llm_roles()
        .unwrap();
    f.close().await;
}

#[tokio::test]
async fn cli_keeps_fragment_layout_and_refuses_to_replace_existing_configuration() {
    let f = Fixture::new().await;
    let target = f.directory.join("cli/server.toml");
    bootstrap::release_example_config(&target).unwrap();
    let profiles = target.with_extension("d").join("profiles/llm.toml");
    assert!(profiles.is_file());
    assert_example_roles(&ServerConfig::load(&target).unwrap());
    let base_before = std::fs::read(&target).unwrap();
    let profiles_before = std::fs::read(&profiles).unwrap();
    assert!(bootstrap::release_example_config(&target).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), base_before);
    assert_eq!(std::fs::read(&profiles).unwrap(), profiles_before);

    let target_with_fragments = f.directory.join("existing.toml");
    let existing_fragments = target_with_fragments.with_extension("d");
    std::fs::create_dir(&existing_fragments).unwrap();
    let operator_file = existing_fragments.join("operator.toml");
    std::fs::write(&operator_file, "# operator configuration\n").unwrap();
    assert!(bootstrap::release_example_config(&target_with_fragments).is_err());
    assert!(!target_with_fragments.exists());
    assert_eq!(
        std::fs::read_to_string(&operator_file).unwrap(),
        "# operator configuration\n"
    );
    f.close().await;
}
