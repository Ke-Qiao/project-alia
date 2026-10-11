//! Isolated real HTTP publication failures and loader recovery; no power-loss claims.
use super::*;
use crate::config::publication::testing::{Stage, fail_once};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;

fn with_port(template: &str, port: u16) -> String {
    let mut config: toml::Value = toml::from_str(template).unwrap();
    config["server"]["port"] = toml::Value::Integer(port.into());
    toml::to_string(&config).unwrap()
}

fn assert_private_base(target: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[cfg(not(unix))]
    let _ = target;
}

fn tree_bytes(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn collect(root: &Path, path: &Path, output: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(root, &path, output);
            } else {
                output.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                ));
            }
        }
    }
    let mut files = Vec::new();
    collect(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

#[tokio::test]
async fn http_prepublication_sync_failure_preserves_old_files_and_allows_retry() {
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        let template = f.template().await;
        let target = f.directory.join("server.toml");
        std::fs::write(&target, &template).unwrap();
        let fragments = target.with_extension("d");
        std::fs::create_dir(&fragments).unwrap();
        let overlay = fragments.join("operator.toml");
        let overlay_content = "[profiles.llm.major]\nmodel = 'operator-kept-model'\n";
        std::fs::write(&overlay, overlay_content).unwrap();
        let proposed = with_port(&template, 45671);
        let fault = fail_once(&target, Stage::FileSync);

        let failed = f.save(&target, &proposed).await;
        assert_eq!(failed["success"], false, "{failed}");
        assert!(failed["error"].is_string());
        assert!(matches!(&*f.done.borrow(), SetupState::Idle));
        assert!(!f.completion.is_pending());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), template);
        assert_eq!(std::fs::read_to_string(&overlay).unwrap(), overlay_content);
        assert!(
            !fault
                .events()
                .iter()
                .any(|event| event.stage == Stage::Published)
        );

        let retried = f.save(&target, &proposed).await;
        assert_eq!(retried["success"], true, "{retried}");
        f.assert_ready(&target);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), proposed);
        assert_eq!(std::fs::read_to_string(&overlay).unwrap(), overlay_content);
        let loaded = ServerConfig::load(&target).unwrap();
        assert_eq!(loaded.server.port, 45671);
        assert_eq!(
            loaded.profiles.llm_profile("major").unwrap().model,
            "operator-kept-model"
        );
        loaded.validate_llm_roles().unwrap();
        assert_private_base(&target);
        f.assert_no_temporary_files();
    })
    .catch_unwind()
    .await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn http_postpublication_failure_reserves_the_original_intent_until_sync_retry() {
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        let template = f.template().await;
        let target = f.directory.join("server.toml");
        std::fs::write(&target, &template).unwrap();
        let proposed = with_port(&template, 45672);
        let fault = fail_once(&target, Stage::Published);

        let failed = f.save(&target, &proposed).await;
        assert_eq!(failed["success"], false, "{failed}");
        assert_eq!(failed["path"], target.to_string_lossy().as_ref());
        let error = failed["error"].as_str().unwrap();
        assert!(error.contains("requires recovery") && error.contains("durability"));
        assert!(matches!(&*f.done.borrow(), SetupState::Pending(_)));
        assert!(f.completion.is_pending());
        assert!(!f.done.borrow().is_terminal());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), proposed);
        assert_private_base(&target);

        let other = f.directory.join("other/server.toml");
        assert_eq!(f.save(&other, &proposed).await["success"], false);
        assert!(!other.parent().unwrap().exists());
        assert_eq!(
            f.save(&target, &with_port(&template, 45673)).await["success"],
            false
        );
        assert!(f.completion.generate(&target, &f.stop).is_err());
        assert!(f.completion.generate(&other, &f.stop).is_err());
        assert!(!other.exists());
        assert!(!target.with_extension("d").exists());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), proposed);
        assert_eq!(
            fault
                .events()
                .iter()
                .filter(|event| event.stage == Stage::Published)
                .count(),
            1
        );

        let retried = f.save(&target, &proposed).await;
        assert_eq!(retried["success"], true, "{retried}");
        f.assert_ready(&target);
        assert!(!f.completion.is_pending());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), proposed);
        assert_eq!(
            fault
                .events()
                .iter()
                .filter(|event| event.stage == Stage::Published)
                .count(),
            1,
            "retry must finish synchronization without republishing the base"
        );
        assert_eq!(ServerConfig::load(&target).unwrap().server.port, 45672);
        f.assert_no_temporary_files();
    })
    .catch_unwind()
    .await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn cli_postpublication_failure_keeps_its_fragments_and_blocks_http_until_retry() {
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        let template = f.template().await;
        let target = f.directory.join("nested/cli/server.toml");
        let fault = fail_once(&target, Stage::Published);
        assert!(f.completion.generate(&target, &f.stop).is_err());
        assert!(matches!(&*f.done.borrow(), SetupState::Pending(_)));
        assert!(target.is_file());
        let base = std::fs::read(&target).unwrap();
        let fragments = target.with_extension("d");
        assert!(fragments.join("profiles/llm.toml").is_file());
        assert!(
            fragments
                .join("resources/appraisal_lexicons/en.toml")
                .is_file()
        );
        let published_fragments = tree_bytes(&fragments);
        assert!(!published_fragments.is_empty());
        assert_private_base(&target);

        let other = f.directory.join("http.toml");
        assert_eq!(f.save(&other, &template).await["success"], false);
        assert!(!other.exists());
        assert_eq!(std::fs::read(&target).unwrap(), base);
        assert_eq!(tree_bytes(&fragments), published_fragments);
        assert_eq!(f.completion.generate(&target, &f.stop).unwrap(), target);
        f.assert_ready(&target);
        assert_eq!(std::fs::read(&target).unwrap(), base);
        assert_eq!(tree_bytes(&fragments), published_fragments);
        assert_eq!(
            fault
                .events()
                .iter()
                .filter(|event| event.stage == Stage::Published)
                .count(),
            1,
            "CLI recovery must retain its original published base and tree"
        );
        assert_example_roles(&ServerConfig::load(&target).unwrap());
    })
    .catch_unwind()
    .await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

struct RetainedDirectory(PathBuf);
impl Drop for RetainedDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn loader_recovers_the_original_publication_without_the_previous_setup_owner() {
    for stage in [Stage::MarkerDurable, Stage::Published] {
        let mut f = Fixture::new().await;
        let target = f.directory.join("server.toml");
        let result = AssertUnwindSafe(async {
            let template = f.template().await;
            std::fs::write(&target, &template).unwrap();
            let proposed = with_port(&template, 45674);
            let fault = fail_once(&target, stage);
            let failed = f.save(&target, &proposed).await;
            assert_eq!(failed["success"], false, "{failed}");
            let error = failed["error"].as_str().unwrap();
            assert!(error.contains("requires recovery") && error.contains("durability"));
            assert!(matches!(&*f.done.borrow(), SetupState::Pending(_)));
            assert_eq!(
                std::fs::read_to_string(&target).unwrap(),
                if stage == Stage::MarkerDurable {
                    &template
                } else {
                    &proposed
                }
                .as_str()
            );
            if stage == Stage::MarkerDurable {
                f.completion.cancel();
                assert!(matches!(&*f.done.borrow(), SetupState::Cancelled));
                let other = f.directory.join("cancelled-attempt.toml");
                assert_eq!(f.save(&other, &proposed).await["success"], false);
                assert!(!other.exists());
            }
            (proposed, fault)
        })
        .catch_unwind()
        .await;
        // Drop the HTTP service, completion owner and watch receiver, retaining only files.
        let _files = RetainedDirectory(std::mem::take(&mut f.directory));
        f.close().await;
        let (proposed, fault) = match result {
            Ok(publication) => publication,
            Err(panic) => std::panic::resume_unwind(panic),
        };

        let recovered = ServerConfig::load(&target).unwrap();
        assert_eq!(recovered.server.port, 45674);
        recovered.validate_llm_roles().unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), proposed);
        assert_private_base(&target);
        assert_eq!(
            fault
                .events()
                .iter()
                .filter(|event| event.stage == Stage::Published)
                .count(),
            1,
            "loader recovery must finish exactly the durable pending publication"
        );
        let events = fault.events().len();
        assert_eq!(ServerConfig::load(&target).unwrap().server.port, 45674);
        assert_eq!(
            fault.events().len(),
            events,
            "loading without a marker must remain read-only"
        );
    }
}
