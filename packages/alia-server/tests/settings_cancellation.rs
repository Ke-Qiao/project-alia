//! Registry future cancellation is separate from HTTP disconnect behavior.
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::time::Duration;

use alia_server::settings::SettingsRegistry;
use futures::FutureExt;
use serde_json::json;

struct SettingsDirectory(PathBuf);

impl SettingsDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("alia-settings-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for SettingsDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn dropping_a_queued_update_does_not_block_the_next_waiter() {
    let settings = SettingsRegistry::new();
    let holder = settings.lock_updates().await;
    let mut cancelled = Box::pin(settings.lock_updates());
    let mut next = Box::pin(settings.lock_updates());
    assert!(futures::poll!(cancelled.as_mut()).is_pending());
    assert!(futures::poll!(next.as_mut()).is_pending());

    // Drop the actual future, not merely a pinned reference to a live waiter.
    drop(cancelled);
    assert!(futures::poll!(next.as_mut()).is_pending());
    drop(holder);
    let _next = tokio::time::timeout(Duration::from_secs(1), next)
        .await
        .expect("the cancelled waiter must not retain update admission");
}

#[tokio::test]
async fn aborting_an_update_holder_releases_admission_without_rolling_back_desired_values() {
    let directory = SettingsDirectory::new();
    let path = directory.0.join("settings.toml");
    let settings = SettingsRegistry::new();
    let key = "plugin.fixture.level";
    settings.set(key, json!(1)).await;
    settings.set_persist_path(path.clone()).await;
    settings.save().await.unwrap();
    let original_file = std::fs::read(&path).unwrap();
    let (held, entered) = tokio::sync::oneshot::channel();
    let updating = settings.clone();
    let mut worker = Some(tokio::spawn(async move {
        let _update = updating.lock_updates().await;
        updating.set(key, json!(2)).await;
        held.send(()).unwrap();
        std::future::pending::<()>().await;
    }));

    let assertions = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(1), entered)
            .await
            .expect("holder must announce its completed desired update")
            .unwrap();
        let mut next = Box::pin(settings.lock_updates());
        assert!(futures::poll!(next.as_mut()).is_pending());
        worker.as_ref().unwrap().abort();
        let cancelled = worker.take().unwrap().await.unwrap_err();
        assert!(cancelled.is_cancelled());
        let _update = tokio::time::timeout(Duration::from_secs(1), next)
            .await
            .expect("aborting a holder must release update admission");

        assert_eq!(settings.get(key).await, Some(json!(2)));
        assert_eq!(std::fs::read(&path).unwrap(), original_file);
        settings.set(key, json!(2)).await;
        settings.save().await.unwrap();
        let reloaded = SettingsRegistry::new();
        reloaded.load_persisted(&path).await.unwrap();
        assert_eq!(reloaded.get(key).await, Some(json!(2)));
    })
    .catch_unwind()
    .await;

    if let Some(worker) = worker.take() {
        worker.abort();
        let _ = worker.await;
    }
    if let Err(error) = assertions {
        std::panic::resume_unwind(error);
    }
}
