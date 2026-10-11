use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::config::{LlmProfileKind, ServerConfig};
use crate::oobe::bootstrap::{SetupCompletion, SetupState};

mod durability;

struct Fixture {
    directory: PathBuf,
    url: String,
    client: reqwest::Client,
    done: watch::Receiver<SetupState>,
    completion: Arc<SetupCompletion>,
    stop: CancellationToken,
    serving: JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("alia-oobe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let completion = Arc::new(SetupCompletion::new());
        let done = completion.subscribe();
        let stop = CancellationToken::new();
        let app = routes().with_state(OobeState {
            completion: completion.clone(),
            cancel: stop.clone(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
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
            completion,
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
        let response = self
            .client
            .post(format!("{}/api/v1/setup/save-config", self.url))
            .json(&json!({"path":path,"config":config}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.json().await.unwrap()
    }

    fn assert_ready(&self, target: &Path) {
        let done = self.done.borrow();
        let SetupState::Ready(path) = &*done else {
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
        assert!(matches!(&*f.done.borrow(), SetupState::Idle));
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
        assert!(matches!(&*f.done.borrow(), SetupState::Idle));
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
    assert!(matches!(&*f.done.borrow(), SetupState::Idle));
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
    assert!(matches!(&*f.done.borrow(), SetupState::Idle));
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
    assert!(matches!(&*f.done.borrow(), SetupState::Idle));
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
    for role in ["memory_capture", "sentence_expression"] {
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
    SetupCompletion::new().generate(&target, &f.stop).unwrap();
    let profiles = target.with_extension("d").join("profiles/llm.toml");
    assert!(profiles.is_file());
    assert_example_roles(&ServerConfig::load(&target).unwrap());
    let base_before = std::fs::read(&target).unwrap();
    let profiles_before = std::fs::read(&profiles).unwrap();
    assert!(SetupCompletion::new().generate(&target, &f.stop).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), base_before);
    assert_eq!(std::fs::read(&profiles).unwrap(), profiles_before);

    let target_with_fragments = f.directory.join("existing.toml");
    let existing_fragments = target_with_fragments.with_extension("d");
    std::fs::create_dir(&existing_fragments).unwrap();
    let operator_file = existing_fragments.join("operator.toml");
    std::fs::write(&operator_file, "# operator configuration\n").unwrap();
    assert!(
        SetupCompletion::new()
            .generate(&target_with_fragments, &f.stop)
            .is_err()
    );
    assert!(!target_with_fragments.exists());
    assert_eq!(
        std::fs::read_to_string(&operator_file).unwrap(),
        "# operator configuration\n"
    );
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_http_saves_publish_one_configuration_and_one_completion() {
    let mut f = Fixture::new().await;
    let template = f.template().await;
    let first = f.directory.join("first.toml");
    let second = f.directory.join("second.toml");
    let (a, b) = tokio::join!(f.save(&first, &template), f.save(&second, &template));
    assert_ne!(
        a["success"], b["success"],
        "exactly one save must win: {a}, {b}"
    );
    let (winner, loser) = if a["success"] == true {
        (&first, &second)
    } else {
        (&second, &first)
    };
    f.assert_ready(winner);
    assert_eq!(std::fs::read_to_string(winner).unwrap(), template);
    assert!(
        !loser.exists(),
        "rejected completion must not write another path"
    );
    f.done.borrow_and_update();
    let late_cli = f.directory.join("late-cli.toml");
    assert!(f.completion.generate(&late_cli, &f.stop).is_err());
    assert!(
        !late_cli.exists(),
        "a late CLI must not publish another config"
    );
    assert!(!late_cli.with_extension("d").exists());
    assert!(!f.done.has_changed().unwrap());
    assert_eq!(f.save(winner, "[broken").await["success"], false);
    assert_eq!(std::fs::read_to_string(winner).unwrap(), template);
    f.assert_no_temporary_files();
    f.close().await;
}

#[tokio::test]
async fn cli_completion_or_cancellation_prevents_late_http_writes() {
    for cancelled in [false, true] {
        let mut f = Fixture::new().await;
        let cli_target = f.directory.join("cli.toml");
        let http_target = f.directory.join("http.toml");
        let template = f.template().await;
        if cancelled {
            f.completion.cancel();
        } else {
            f.completion.generate(&cli_target, &f.stop).unwrap();
        }
        f.done.borrow_and_update();
        let result = f.save(&http_target, &template).await;
        assert_eq!(result["success"], false, "{result}");
        assert!(!http_target.exists());
        assert!(!f.done.has_changed().unwrap());
        if cancelled {
            assert!(!cli_target.exists());
        } else {
            f.assert_ready(&cli_target);
        }
        f.close().await;
    }
}

#[tokio::test]
async fn setup_rejects_foreign_hosts_origins_and_non_json_without_writing() {
    let f = Fixture::new().await;
    let target = f.directory.join("server.toml");
    let template = f.template().await;
    let url = format!("{}/api/v1/setup/save-config", f.url);
    let body = json!({"path": target, "config": template});
    for (host, origin) in [
        (Some("rebind.example"), None),
        (Some("127.0.0.1.rebind.example"), None),
        (Some("127.0.0.2"), None),
        (None, Some("https://outside.example")),
        (None, Some("null")),
        (None, Some("http://localhost")),
    ] {
        let mut request = f.client.post(&url).json(&body);
        if let Some(host) = host {
            request = request.header("Host", host);
        }
        if let Some(origin) = origin {
            request = request.header("Origin", origin);
        }
        assert_eq!(
            request.send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert!(!target.exists());
        assert!(matches!(&*f.done.borrow(), SetupState::Idle));
    }
    assert_eq!(
        f.client
            .post(&url)
            .header("Content-Type", "text/plain")
            .body(body.to_string())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert!(!target.exists());
    let result: Value = f
        .client
        .post(&url)
        .header("Origin", &f.url)
        .json(&body)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        result["success"], true,
        "same-origin JSON is allowed: {result}"
    );
    f.assert_ready(&target);
    f.close().await;
}
