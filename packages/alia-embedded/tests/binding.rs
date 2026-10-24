use alia_embedded::binding::{BindingManager, BindingState};
use alia_embedded::storage::EmbeddedStore;
use alia_transport::{
    bind_payload, current_unix_timestamp, handoff_completion_payload, handoff_intent_payload,
    sign_payload,
};
use ed25519_dalek::SigningKey;

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

#[tokio::test]
async fn pending_handoff_cannot_be_replaced_by_bind_and_restart_retains_original_server() {
    for restart in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = test_manager(&dir).await;
        let original = SigningKey::from_bytes(&[1; 32]);
        let replacement = SigningKey::from_bytes(&[2; 32]);
        let original_key = original.verifying_key().to_bytes();
        let replacement_key = replacement.verifying_key().to_bytes();
        let endpoint = "http://127.0.0.1:3000";
        let issued_at = current_unix_timestamp().unwrap();
        manager
            .bind(
                "original",
                "original",
                endpoint,
                &original_key,
                &sign_payload(
                    &original,
                    &bind_payload("original", "original", endpoint, &original_key, issued_at),
                ),
                issued_at,
            )
            .await
            .unwrap();
        let (token, _) = manager
            .prepare_handoff(
                "original",
                "replacement",
                "replacement",
                endpoint,
                &replacement_key,
                &sign_payload(
                    &original,
                    &handoff_intent_payload(
                        "original",
                        "replacement",
                        "replacement",
                        endpoint,
                        &replacement_key,
                        issued_at,
                    ),
                ),
                issued_at,
            )
            .await
            .unwrap();
        assert_eq!(manager.status().await.state, BindingState::HandoffPending);
        if restart {
            manager = test_manager(&dir).await;
            manager.load().await.unwrap();
        }
        // Even the intended target must use the authorized handoff, never a fresh Bind.
        assert!(
            manager
                .bind(
                    "replacement",
                    "replacement",
                    endpoint,
                    &replacement_key,
                    &sign_payload(
                        &replacement,
                        &bind_payload(
                            "replacement",
                            "replacement",
                            endpoint,
                            &replacement_key,
                            issued_at
                        ),
                    ),
                    issued_at,
                )
                .await
                .is_err()
        );
        let status = manager.status().await;
        assert_eq!(status.server_id.as_deref(), Some("original"));
        assert_eq!(
            status.server_public_key.as_deref(),
            Some(original_key.as_slice())
        );
        assert_eq!(
            status.state,
            if restart {
                BindingState::Bound
            } else {
                BindingState::HandoffPending
            }
        );
        let completed = manager
            .complete_handoff(
                &token,
                "replacement",
                "replacement",
                endpoint,
                &replacement_key,
                &sign_payload(
                    &replacement,
                    &handoff_completion_payload(
                        &token,
                        "replacement",
                        "replacement",
                        endpoint,
                        &replacement_key,
                        issued_at,
                    ),
                ),
                issued_at,
            )
            .await;
        if restart {
            assert!(completed.is_err());
            let reloaded = test_manager(&dir).await;
            reloaded.load().await.unwrap();
            let status = reloaded.status().await;
            assert_eq!(status.state, BindingState::Bound);
            assert_eq!(status.server_id.as_deref(), Some("original"));
        } else {
            completed.unwrap();
            assert_eq!(
                manager.status().await.server_id.as_deref(),
                Some("replacement")
            );
        }
    }
}
