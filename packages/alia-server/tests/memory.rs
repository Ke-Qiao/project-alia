#![cfg(feature = "memory-graphrag")]

use alia_server::providers::adapter::{MemoryNode, MemoryPatch, RetrievalOpts};
use alia_server::providers::memory::graphrag::{GraphRagConfig, GraphRagMemory};
use alia_server::providers::memory::simple::SimpleMemory;
use alia_server::providers::memory::working::{ConversationTurn, WorkingMemory};
use alia_server::providers::MemoryBackend;
use alia_server::entity::memory_node::MemoryLayer;
use chrono::Utc;
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
        content: "用户喜欢喝咖啡".to_string(),
        metadata: serde_json::json!({}),
        importance: 0.8,
    };

    let id = memory.store(user_id, node).await.unwrap();
    assert!(!id.is_nil());

    let results = memory
        .retrieve(user_id, "咖啡", RetrievalOpts::default())
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("咖啡"));
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
            id,
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
    memory.delete(id).await.unwrap();

    let results = memory
        .retrieve(user_id, "updated", RetrievalOpts::default())
        .await
        .unwrap();
    assert!(results.is_empty());
}

// ── WorkingMemory ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_add_and_get_turns() {
    let memory = WorkingMemory::new(5);
    let user_id = Uuid::new_v4();

    for i in 0..3 {
        let turn = ConversationTurn {
            id: Uuid::new_v4(),
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            content: format!("Message {}", i),
            timestamp: Utc::now(),
            metadata: serde_json::Value::Null,
        };
        memory.add_turn(user_id, turn).await;
    }

    let messages = memory.get_messages(user_id, None).await;
    assert_eq!(messages.len(), 3);
}

#[tokio::test]
async fn test_window_overflow() {
    let memory = WorkingMemory::new(3);
    let user_id = Uuid::new_v4();

    for i in 0..5 {
        let turn = ConversationTurn {
            id: Uuid::new_v4(),
            role: "user".into(),
            content: format!("Message {}", i),
            timestamp: Utc::now(),
            metadata: serde_json::Value::Null,
        };
        let is_full = memory.add_turn(user_id, turn).await;
        if i >= 2 {
            assert!(is_full);
        }
    }

    let messages = memory.get_messages(user_id, None).await;
    assert_eq!(messages.len(), 3);
    assert!(messages[0].content.contains("2"));
}

#[tokio::test]
async fn test_drain_for_summary() {
    let memory = WorkingMemory::new(10);
    let user_id = Uuid::new_v4();

    for i in 0..5 {
        let turn = ConversationTurn {
            id: Uuid::new_v4(),
            role: "user".into(),
            content: format!("Message {}", i),
            timestamp: Utc::now(),
            metadata: serde_json::Value::Null,
        };
        memory.add_turn(user_id, turn).await;
    }

    let drained = memory.drain_for_summary(user_id).await;
    assert_eq!(drained.len(), 5);

    let messages = memory.get_messages(user_id, None).await;
    assert_eq!(messages.len(), 0);
}

// ── GraphRag config ────────────────────────────────────────────────────

#[test]
fn test_default_config() {
    let config = GraphRagConfig::default();
    assert_eq!(config.working_window_size, 20);
    assert_eq!(config.episodic_ttl_days, 90);
    assert_eq!(config.max_core_memories, 500);
}

#[test]
fn test_layer_conversion() {
    assert!(matches!(
        GraphRagMemory::adapter_to_layer("episodic"),
        MemoryLayer::Episodic
    ));
    assert!(matches!(
        GraphRagMemory::adapter_to_layer("core"),
        MemoryLayer::Core
    ));
    assert!(matches!(
        GraphRagMemory::adapter_to_layer("procedural"),
        MemoryLayer::Procedural
    ));
}
