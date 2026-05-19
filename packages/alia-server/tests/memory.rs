use alia_server::providers::MemoryBackend;
use alia_server::providers::adapter::{MemoryListOpts, MemoryNode, MemoryPatch, RetrievalOpts};
use alia_server::providers::memory::NoopMemory;
use alia_server::providers::memory::simple::SimpleMemory;
use uuid::Uuid;

// ── SimpleMemory ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_store_and_retrieve() {
    let memory = SimpleMemory::new();
    let user_id = Uuid::new_v4();

    let node = MemoryNode {
        id: None,
        memory_layer: "core".to_string(),
        node_type: "fact".to_string(),
        content: "User likes coffee".to_string(),
        metadata: serde_json::json!({}),
        importance: 0.8,
    };

    let id = memory.store(user_id, node).await.unwrap();
    assert!(!id.is_empty());

    let results = memory
        .retrieve(user_id, "coffee", RetrievalOpts::default())
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("coffee"));
}

#[tokio::test]
async fn test_update_and_delete() {
    let memory = SimpleMemory::new();
    let user_id = Uuid::new_v4();

    let node = MemoryNode {
        id: None,
        memory_layer: "core".to_string(),
        node_type: "fact".to_string(),
        content: "original content".to_string(),
        metadata: serde_json::json!({}),
        importance: 0.5,
    };

    let id = memory.store(user_id, node).await.unwrap();

    // Update
    memory
        .update(
            &id,
            MemoryPatch {
                content: Some("updated content".to_string()),
                metadata: None,
                importance: Some(0.9),
            },
        )
        .await
        .unwrap();

    let results = memory
        .retrieve(user_id, "updated", RetrievalOpts::default())
        .await
        .unwrap();
    assert_eq!(results[0].content, "updated content");
    assert_eq!(results[0].importance, 0.9);

    // Delete
    memory.delete(&id).await.unwrap();

    let results = memory
        .retrieve(user_id, "updated", RetrievalOpts::default())
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_list_get_and_clear() {
    let memory = SimpleMemory::new();
    let user_id = Uuid::new_v4();

    let first = memory
        .store(
            user_id,
            MemoryNode {
                id: None,
                memory_layer: "core".to_string(),
                node_type: "preference".to_string(),
                content: "User prefers warm light".to_string(),
                metadata: serde_json::json!({}),
                importance: 0.9,
            },
        )
        .await
        .unwrap();
    memory
        .store(
            user_id,
            MemoryNode {
                id: None,
                memory_layer: "episodic".to_string(),
                node_type: "event".to_string(),
                content: "User tested microphone volume".to_string(),
                metadata: serde_json::json!({}),
                importance: 0.4,
            },
        )
        .await
        .unwrap();

    let listed = memory
        .list(
            user_id,
            MemoryListOpts {
                max_results: Some(10),
                layers: Some(vec!["core".to_string()]),
            },
        )
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id.as_deref(), Some(first.as_str()));

    let found = memory.get(user_id, &first).await.unwrap().unwrap();
    assert_eq!(found.content, "User prefers warm light");

    assert_eq!(memory.clear(user_id).await.unwrap(), 2);
    assert!(
        memory
            .list(user_id, MemoryListOpts::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn memory_capabilities_are_provider_neutral() {
    let simple = SimpleMemory::new();
    let simple_caps = simple.memory_capabilities().await;
    assert_eq!(simple_caps.abi_version, 1);
    assert!(simple_caps.supports_store);
    assert!(simple_caps.supports_layer_filters);
    assert!(simple_caps.supports_similarity_threshold);
    assert!(!simple_caps.supports_provider_infer);

    let noop = NoopMemory;
    let noop_caps = noop.memory_capabilities().await;
    assert!(!noop_caps.supports_store);
    assert!(!noop_caps.supports_retrieve);
}
