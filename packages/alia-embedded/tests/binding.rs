use alia_embedded::binding::{BindingManager, BindingState};
use alia_embedded::storage::EmbeddedStore;

async fn test_manager(dir: &tempfile::TempDir) -> BindingManager {
    let store = EmbeddedStore::open(dir.path().join("embedded-state.sqlite3"))
        .await
        .unwrap();
    BindingManager::new(store, 3, 60)
}

#[tokio::test]
async fn persists_binding_state_to_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let manager = test_manager(&dir).await;

    manager
        .update_for_test(BindingState::Bound, Some("server-1".to_string()))
        .await;
    manager.persist().await.unwrap();

    let reloaded = test_manager(&dir).await;
    reloaded.load().await.unwrap();
    let status = reloaded.status().await;

    assert_eq!(status.state, BindingState::Bound);
    assert_eq!(status.server_id.as_deref(), Some("server-1"));
}
