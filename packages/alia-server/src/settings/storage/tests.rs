use super::*;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone)]
struct Event {
    stage: Stage,
    path: PathBuf,
}
enum Action {
    Fail,
    Pause,
}
struct Hook {
    action: Mutex<Option<(Stage, Action)>>,
    events: Mutex<Vec<Event>>,
    entered: tokio::sync::Semaphore,
    released: Mutex<bool>,
    wake: Condvar,
}
fn hooks() -> &'static Mutex<HashMap<PathBuf, Arc<Hook>>> {
    static HOOKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Hook>>>> = OnceLock::new();
    HOOKS.get_or_init(Mutex::default)
}
struct Guard {
    target: PathBuf,
    hook: Arc<Hook>,
}
impl Guard {
    fn new(target: &Path, action: Option<(Stage, Action)>) -> Self {
        let hook = Arc::new(Hook {
            action: Mutex::new(action),
            events: Mutex::default(),
            entered: tokio::sync::Semaphore::new(0),
            released: Mutex::new(false),
            wake: Condvar::new(),
        });
        let mut all = hooks().lock().unwrap();
        assert!(!all.contains_key(target));
        all.insert(target.to_path_buf(), hook.clone());
        Self {
            target: target.to_path_buf(),
            hook,
        }
    }
    fn release(&self) {
        *self.hook.released.lock().unwrap() = true;
        self.hook.wake.notify_all();
    }
    async fn entered(&self) {
        self.hook.entered.acquire().await.unwrap().forget();
    }
    fn events(&self) -> Vec<Event> {
        self.hook.events.lock().unwrap().clone()
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.release();
        hooks().lock().unwrap().remove(&self.target);
    }
}
pub(super) fn checkpoint(target: &Path, stage: Stage, path: &Path) -> Result<()> {
    let hook = hooks().lock().unwrap().get(target).cloned();
    let Some(hook) = hook else {
        return Ok(());
    };
    hook.events.lock().unwrap().push(Event {
        stage,
        path: path.to_path_buf(),
    });
    let action = {
        let mut action = hook.action.lock().unwrap();
        if action
            .as_ref()
            .is_some_and(|(expected, _)| *expected == stage)
        {
            action.take()
        } else {
            None
        }
    };
    match action {
        Some((_, Action::Fail)) => anyhow::bail!("injected settings {stage:?} failure"),
        Some((_, Action::Pause)) => {
            hook.entered.add_permits(1);
            let mut released = hook.released.lock().unwrap();
            while !*released {
                released = hook.wake.wait(released).unwrap();
            }
        }
        None => {}
    }
    Ok(())
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("alia-settings-storage-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn target(&self) -> PathBuf {
        self.0.join("settings.toml")
    }
    fn assert_no_temporary(&self) {
        assert!(fs::read_dir(&self.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".alia-settings-")
        }));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn preparation_failure_keeps_the_complete_previous_snapshot_and_removes_candidate() {
    let f = Fixture::new();
    let target = f.target();
    fs::write(&target, "value = 'old'\n").unwrap();
    let guard = Guard::new(&target, Some((Stage::FileSync, Action::Fail)));
    let error = save(&target, "value = 'new'\n").unwrap_err();
    assert!(!error.was_published());
    assert_eq!(fs::read_to_string(&target).unwrap(), "value = 'old'\n");
    assert!(
        !guard
            .events()
            .iter()
            .any(|event| event.stage == Stage::Published)
    );
    f.assert_no_temporary();
}

#[test]
fn published_failure_reports_unknown_and_explicit_retry_syncs_the_complete_parent_chain() {
    let f = Fixture::new();
    let target = f.target();
    fs::write(&target, "value = 'old'\n").unwrap();
    let guard = Guard::new(&target, Some((Stage::DirectorySync, Action::Fail)));
    let error = save(&target, "value = 'new'\n").unwrap_err();
    assert!(error.was_published());
    assert!(error.to_string().contains("durability is unknown"));
    assert_eq!(fs::read_to_string(&target).unwrap(), "value = 'new'\n");
    f.assert_no_temporary();
    let retried = guard.events().len();
    save(&target, "value = 'new'\n").unwrap();
    let events = guard.events();
    let events = &events[retried..];
    let file_synced = events
        .iter()
        .position(|event| event.stage == Stage::FileSync)
        .unwrap();
    let prepared = events
        .iter()
        .position(|event| event.stage == Stage::Prepared)
        .unwrap();
    let published = events
        .iter()
        .position(|event| event.stage == Stage::Published)
        .unwrap();
    assert!(file_synced < prepared && prepared < published);
    for ancestor in target.parent().unwrap().ancestors() {
        assert!(
            events[published + 1..]
                .iter()
                .any(|event| event.stage == Stage::DirectorySync && event.path == ancestor)
        );
    }
}

#[test]
fn later_save_syncs_parents_left_visible_by_an_earlier_prepare_failure() {
    let f = Fixture::new();
    let target = f.0.join("nested/inner/settings.toml");
    let guard = Guard::new(&target, Some((Stage::FileSync, Action::Fail)));
    assert!(!save(&target, "value = 1").unwrap_err().was_published());
    assert!(target.parent().unwrap().is_dir());
    let retried = guard.events().len();
    save(&target, "value = 2").unwrap();
    let events = guard.events();
    for ancestor in target.parent().unwrap().ancestors() {
        assert!(
            events[retried..]
                .iter()
                .any(|event| event.stage == Stage::DirectorySync && event.path == ancestor)
        );
    }
}

#[test]
fn a_non_regular_target_is_rejected_before_opening_for_write() {
    let f = Fixture::new();
    let target = f.target();
    fs::create_dir(&target).unwrap();
    let error = save(&target, "value = 1").unwrap_err();
    assert!(!error.was_published());
    assert!(error.to_string().contains("regular file"));
    assert!(target.is_dir());
    f.assert_no_temporary();
}

#[cfg(unix)]
#[test]
fn private_new_files_and_existing_modes_are_preserved_without_replacing_hardlinks() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let f = Fixture::new();
    let target = f.target();
    save(&target, "value = 'private'").unwrap();
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o600);
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    let original = fs::metadata(&target).unwrap();
    save(&target, "value = 'replacement'").unwrap();
    let replaced = fs::metadata(&target).unwrap();
    assert_eq!(replaced.mode() & 0o777, 0o640);
    assert_eq!(
        (replaced.uid(), replaced.gid()),
        (original.uid(), original.gid())
    );
    let alias = f.0.join("hardlink.toml");
    fs::hard_link(&target, &alias).unwrap();
    let error = save(&target, "value = 'must-not-change'").unwrap_err();
    assert!(!error.was_published());
    assert!(error.to_string().contains("multiple hard links"));
    assert_eq!(fs::read_to_string(&alias).unwrap(), "value = 'replacement'");
    assert_eq!(fs::metadata(&target).unwrap().ino(), replaced.ino());
    f.assert_no_temporary();
}

#[cfg(unix)]
#[test]
fn valid_and_dangling_symlinks_follow_the_target_without_creating_missing_link_parents() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let target = f.target();
    let alias = f.0.join("alias.toml");
    symlink("settings.toml", &alias).unwrap();
    save(&alias, "value = 'created'").unwrap();
    save(&alias, "value = 'replaced'").unwrap();
    assert_eq!(
        fs::read_link(&alias).unwrap(),
        PathBuf::from("settings.toml")
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "value = 'replaced'");
    let missing = f.0.join("missing-alias.toml");
    symlink("absent/target.toml", &missing).unwrap();
    assert!(!save(&missing, "value = 1").unwrap_err().was_published());
    assert!(!f.0.join("absent").exists());
    assert!(
        fs::symlink_metadata(&missing)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let cycle = f.0.join("cycle.toml");
    symlink("cycle.toml", &cycle).unwrap();
    assert!(!save(&cycle, "value = 1").unwrap_err().was_published());
    f.assert_no_temporary();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queued_save_snapshots_after_admission_without_holding_the_registry_read_lock() {
    use crate::settings::SettingsRegistry;
    use futures::FutureExt;
    use std::panic::AssertUnwindSafe;

    let f = Fixture::new();
    let target = f.target();
    let registry = SettingsRegistry::new();
    registry.set_persist_path(target.clone()).await;
    registry
        .set("plugin.example.value", serde_json::json!("old"))
        .await;
    let guard = Guard::new(&target, Some((Stage::Prepared, Action::Pause)));
    let first_registry = registry.clone();
    let first = tokio::spawn(async move { first_registry.save().await });
    let result = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(3), guard.entered())
            .await
            .unwrap();
        let second = registry.save();
        tokio::pin!(second);
        assert!(futures::poll!(second.as_mut()).is_pending());
        tokio::time::timeout(
            Duration::from_secs(3),
            registry.set("plugin.example.value", serde_json::json!("new")),
        )
        .await
        .unwrap();
        guard.release();
        tokio::time::timeout(Duration::from_secs(3), second)
            .await
            .unwrap()
            .unwrap();
    })
    .catch_unwind()
    .await;
    guard.release();
    tokio::time::timeout(Duration::from_secs(3), first)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    let stored: std::collections::BTreeMap<String, serde_json::Value> =
        toml::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(stored["plugin.example.value"], serde_json::json!("new"));
    assert_eq!(
        guard
            .events()
            .iter()
            .filter(|event| event.stage == Stage::Published)
            .count(),
        2
    );
    f.assert_no_temporary();
}
