use super::testing::{fail_once, pause_once};
use super::*;
use std::time::Duration;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("alia-publication-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn target(&self) -> PathBuf {
        self.0.join("server.toml")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn template() -> String {
    crate::oobe::bootstrap::example_config().unwrap()
}
fn pending(error: anyhow::Error) -> PendingPublication {
    error.downcast::<PendingPublication>().unwrap()
}
fn assert_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[test]
fn invalid_input_and_pre_marker_failure_preserve_unowned_files() {
    let f = Fixture::new();
    let invalid = f.0.join("must-not-exist/server.toml");
    assert!(save(&invalid, "[broken", true, None).is_err());
    assert!(!invalid.parent().unwrap().exists());

    let target = f.target();
    let original = template();
    fs::write(&target, &original).unwrap();
    let fragments = target.with_extension("d");
    fs::create_dir(&fragments).unwrap();
    let overlay = fragments.join("operator.toml");
    fs::write(&overlay, "[server]\nport = 45680\n").unwrap();
    let original_overlay = snapshot(&overlay).unwrap();
    let guard = fail_once(&target, Stage::FileSync);
    let error = save(&target, &format!("{original}\n# proposed"), true, None).unwrap_err();
    assert!(error.downcast_ref::<PendingPublication>().is_none());
    assert_eq!(fs::read_to_string(&target).unwrap(), original);
    assert_eq!(snapshot(&overlay).unwrap(), original_overlay);
    assert!(!has_pending(&target));
    assert!(
        !guard
            .events()
            .iter()
            .any(|event| event.stage == Stage::Published)
    );
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 2);

    let cli = f.0.join("first.toml");
    let _guard = fail_once(&cli, Stage::FileSync);
    let error = save(
        &cli,
        &original,
        false,
        Some(&[("nested/one.toml", "[server]\nport = 45681")]),
    )
    .unwrap_err();
    assert!(error.downcast_ref::<PendingPublication>().is_none());
    assert!(!cli.exists());
    assert!(!cli.with_extension("d").exists());
    assert!(!has_pending(&cli));
}

#[test]
fn retry_syncs_all_ancestors_left_visible_by_an_earlier_preparation_failure() {
    let f = Fixture::new();
    let target = f.0.join("nested/inner/server.toml");
    let content = template();
    let first = fail_once(&target, Stage::FileSync);
    let error = save(&target, &content, true, None).unwrap_err();
    assert!(error.downcast_ref::<PendingPublication>().is_none());
    assert!(target.parent().unwrap().is_dir());
    assert!(!target.exists());
    assert!(!has_pending(&target));
    drop(first);

    let retried = fail_once(&target, Stage::MarkerDurable);
    let publication = pending(save(&target, &content, true, None).unwrap_err());
    let events = retried.events();
    let durable = events
        .iter()
        .position(|event| event.stage == Stage::MarkerDurable)
        .unwrap();
    for ancestor in target.parent().unwrap().ancestors() {
        assert!(
            events[..durable]
                .iter()
                .any(|event| event.stage == Stage::DirectorySync && event.path == ancestor),
            "retry skipped a visible but previously unconfirmed ancestor: {}",
            ancestor.display()
        );
    }
    assert!(!target.exists());
    publication.retry().unwrap();
    assert!(!has_pending(&target));
    assert_eq!(fs::read_to_string(&target).unwrap(), content);
}

#[test]
fn fragment_tree_and_new_ancestors_are_synced_before_the_marker_and_base() {
    let f = Fixture::new();
    let target = f.0.join("new/inner/server.toml");
    let content = format!("{}\n# unique-private-template-data", template());
    let guard = fail_once(&target, Stage::MarkerDurable);
    let publication = pending(
        save(
            &target,
            &content,
            false,
            Some(&[
                ("nested/deep/one.toml", "[server]\nport = 45682"),
                ("two.toml", "[server]\nlog_level = 'info'"),
            ]),
        )
        .unwrap_err(),
    );
    assert!(!target.exists());
    let descriptor = read_descriptor(&target).unwrap();
    let marker = marker_path(&target);
    assert_private(&marker);
    assert_private(&target.parent().unwrap().join(&descriptor.candidate));
    assert!(
        !fs::read_to_string(&marker)
            .unwrap()
            .contains("unique-private-template-data")
    );
    let events = guard.events();
    let durable = events
        .iter()
        .position(|event| event.stage == Stage::MarkerDurable)
        .unwrap();
    let position = |stage, path: &Path| {
        events[..durable]
            .iter()
            .position(|event| event.stage == stage && event.path == path)
            .unwrap()
    };
    let root = target.with_extension("d");
    let deep = root.join("nested/deep");
    assert!(
        position(Stage::FileSync, &deep.join("one.toml")) < position(Stage::DirectorySync, &deep)
    );
    assert!(
        position(Stage::DirectorySync, &deep)
            < position(Stage::DirectorySync, &root.join("nested"))
    );
    assert!(
        position(Stage::DirectorySync, &root.join("nested"))
            < position(Stage::DirectorySync, &root)
    );
    assert!(
        position(Stage::DirectorySync, &root)
            < position(Stage::DirectorySync, target.parent().unwrap())
    );
    assert!(
        position(Stage::DirectorySync, target.parent().unwrap())
            < position(Stage::DirectorySync, &f.0.join("new"))
    );
    assert!(
        position(Stage::DirectorySync, &f.0.join("new")) < position(Stage::DirectorySync, &f.0)
    );
    assert!(publication.matches_request(
        &target,
        &content,
        false,
        Some(&[
            ("nested/deep/one.toml", "[server]\nport = 45682"),
            ("two.toml", "[server]\nlog_level = 'info'"),
        ])
    ));
    publication.retry().unwrap();
    let events = guard.events();
    let published = events
        .iter()
        .position(|event| event.stage == Stage::Published)
        .unwrap();
    let removed = events
        .iter()
        .position(|event| event.stage == Stage::MarkerRemoved)
        .unwrap();
    assert!(durable < published && published < removed);
    assert!(
        events[published + 1..removed]
            .iter()
            .any(|event| event.stage == Stage::DirectorySync && event.path == f.0)
    );
    assert!(
        events[removed + 1..]
            .iter()
            .any(|event| event.stage == Stage::DirectorySync && event.path == f.0)
    );
    assert_private(&target);
    assert!(!has_pending(&target));
    assert_eq!(ServerConfig::load(&target).unwrap().server.port, 45682);
}

#[test]
fn marker_removal_failure_requires_the_in_memory_intent_until_sync_succeeds() {
    let f = Fixture::new();
    let target = f.target();
    let content = template();
    let guard = fail_once(&target, Stage::MarkerRemoved);
    let publication = pending(save(&target, &content, true, None).unwrap_err());
    let published = snapshot(&target).unwrap();
    assert!(!has_pending(&target));
    assert_eq!(
        guard
            .events()
            .iter()
            .filter(|event| event.stage == Stage::Published)
            .count(),
        1
    );
    drop(guard);
    let retry_fault = fail_once(&target, Stage::DirectorySync);
    recover(&target).unwrap();
    ServerConfig::load(&target).unwrap();
    assert!(
        retry_fault.events().is_empty(),
        "ordinary marker-free loading must remain read-only"
    );
    pending(publication.retry().unwrap_err());
    assert!(!has_pending(&target));
    publication.retry().unwrap();
    assert_eq!(snapshot(&target).unwrap(), published);
    assert!(
        retry_fault
            .events()
            .iter()
            .any(|event| event.stage == Stage::DirectorySync)
    );
    assert!(
        !retry_fault
            .events()
            .iter()
            .any(|event| event.stage == Stage::Published)
    );
}

#[test]
fn recovery_rejects_target_candidate_and_fragment_changes_without_overwriting() {
    for change in [
        "target",
        "candidate",
        "content",
        "add",
        "remove",
        "directory",
    ] {
        let f = Fixture::new();
        let target = f.target();
        let content = template();
        fs::write(&target, &content).unwrap();
        let root = target.with_extension("d");
        fs::create_dir(&root).unwrap();
        let fragment = root.join("operator.toml");
        fs::write(&fragment, "[server]\nport = 45683").unwrap();
        let guard = fail_once(&target, Stage::MarkerDurable);
        let publication =
            pending(save(&target, &format!("{content}\n# proposed"), true, None).unwrap_err());
        let candidate = target
            .parent()
            .unwrap()
            .join(&publication.descriptor.candidate);
        match change {
            "target" => fs::write(&target, "operator modified base").unwrap(),
            "candidate" => fs::write(candidate, "operator modified candidate").unwrap(),
            "content" => fs::write(&fragment, "[server]\nport = 45684").unwrap(),
            "add" => fs::write(root.join("extra.toml"), "[server]\nport = 45685").unwrap(),
            "remove" => fs::remove_file(&fragment).unwrap(),
            "directory" => fs::create_dir(root.join("extra")).unwrap(),
            _ => unreachable!(),
        }
        let expected = snapshot(&target).unwrap();
        pending(publication.retry().unwrap_err());
        pending(recover(&target).unwrap_err());
        assert_eq!(snapshot(&target).unwrap(), expected, "{change}");
        assert!(has_pending(&target));
        assert!(
            !guard
                .events()
                .iter()
                .any(|event| event.stage == Stage::Published),
            "{change}"
        );
    }
}

#[test]
fn existing_marker_mismatch_remains_typed_and_does_not_start_another_intent() {
    let f = Fixture::new();
    let target = f.target();
    let content = template();
    let _guard = fail_once(&target, Stage::MarkerDurable);
    pending(save(&target, &content, true, None).unwrap_err());
    let error = pending(save(&target, &format!("{content}\n# different"), true, None).unwrap_err());
    assert!(error.matches_request(&target, &content, true, None));
    assert!(!error.matches_request(&target, &format!("{content}\n# different"), true, None));
    assert!(error.to_string().contains("durability is unknown"));
    assert!(!target.exists());
    error.retry().unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), content);
}

#[test]
fn recovery_rejects_path_escape_and_cleans_only_its_matching_descriptor_temporary() {
    let f = Fixture::new();
    let target = f.target();
    let content = template();
    let _guard = fail_once(&target, Stage::MarkerDurable);
    let publication = pending(save(&target, &content, true, None).unwrap_err());
    let marker = marker_path(&target);
    let outside = f.0.join("operator-data");
    fs::write(&outside, "retain").unwrap();
    let mut invalid = publication.descriptor.clone();
    invalid.candidate = "../operator-data".into();
    fs::write(&marker, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(recover(&target).is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "retain");
    assert!(!target.exists());
    fs::write(
        &marker,
        serde_json::to_vec(&publication.descriptor).unwrap(),
    )
    .unwrap();
    let temporary = intent_temporary(&target, &publication.descriptor);
    fs::hard_link(&marker, &temporary).unwrap();
    publication.retry().unwrap();
    assert!(!temporary.exists());
    assert!(!marker.exists());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "retain");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_exclusive_marker_conflict_never_removes_the_other_record() {
    let f = Fixture::new();
    let target = f.target();
    let content = template();
    let guard = pause_once(&target, Stage::FileSync);
    let worker_target = target.clone();
    let mut worker =
        tokio::task::spawn_blocking(move || save(&worker_target, &content, true, None));
    let entered = tokio::time::timeout(Duration::from_secs(3), guard.entered()).await;
    if entered.is_err() {
        guard.release();
        let _ = tokio::time::timeout(Duration::from_secs(3), &mut worker).await;
        panic!("publication did not enter its controlled file sync");
    }
    let marker = marker_path(&target);
    let write = fs::write(&marker, "another publisher's record");
    guard.release();
    let outcome = tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    write.unwrap();
    assert!(outcome.is_err());
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        "another publisher's record"
    );
    assert!(!target.exists());
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn a_base_symlink_keeps_its_canonical_overlay_but_a_candidate_symlink_is_rejected() {
    use std::os::unix::fs::symlink;

    let f = Fixture::new();
    let target = f.0.join("actual/server.toml");
    fs::create_dir(target.parent().unwrap()).unwrap();
    let content = template();
    fs::write(&target, &content).unwrap();
    let root = target.with_extension("d");
    fs::create_dir(&root).unwrap();
    let overlay = root.join("operator.toml");
    fs::write(&overlay, "[server]\nport = 45687").unwrap();
    let original_overlay = snapshot(&overlay).unwrap();
    let alias = f.0.join("alias.toml");
    symlink(&target, &alias).unwrap();
    let proposed = format!("{content}\n# through existing symlink");
    assert_eq!(save(&alias, &proposed, true, None).unwrap(), alias);
    assert!(
        fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_link(&alias).unwrap(), target);
    assert_eq!(fs::read_to_string(&target).unwrap(), proposed);
    assert_eq!(snapshot(&overlay).unwrap(), original_overlay);
    assert_eq!(ServerConfig::load(&alias).unwrap().server.port, 45687);
    assert!(!alias.with_extension("d").exists());

    let _guard = fail_once(&alias, Stage::MarkerDurable);
    let pending = pending(save(&alias, &format!("{content}\n# next"), true, None).unwrap_err());
    let candidate = target.parent().unwrap().join(&pending.descriptor.candidate);
    let external = f.0.join("external-file");
    fs::write(&external, fs::read(&candidate).unwrap()).unwrap();
    let external_snapshot = snapshot(&external).unwrap();
    fs::remove_file(&candidate).unwrap();
    symlink(&external, &candidate).unwrap();
    let error = pending.retry().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("candidate must remain a regular file")
    );
    assert_eq!(snapshot(&external).unwrap(), external_snapshot);
    assert_eq!(fs::read_to_string(&target).unwrap(), proposed);
    assert!(
        fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(has_pending(&alias));
}
