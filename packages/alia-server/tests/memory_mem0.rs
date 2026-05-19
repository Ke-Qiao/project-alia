use std::time::Duration;

use alia_server::providers::adapter::MemoryNode;
use alia_server::providers::memory::external::ExternalMemoryClient;
use alia_server::providers::memory::mem0::{
    Mem0Client, Mem0Config, extract_memory_id, parse_memory_nodes,
};
use uuid::Uuid;

#[test]
fn parses_search_results_with_alia_metadata() {
    let value = serde_json::json!({
        "results": [
            {
                "id": "mem_1",
                "memory": "KeQiao likes tsukuyomi",
                "metadata": {
                    "alia_memory_layer": "core",
                    "alia_node_type": "preference",
                    "alia_importance": 0.9
                }
            }
        ]
    });

    let nodes = parse_memory_nodes(&value);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].id.as_deref(), Some("mem_1"));
    assert_eq!(nodes[0].memory_layer, "core");
    assert_eq!(nodes[0].node_type, "preference");
    assert_eq!(nodes[0].importance, 0.9);
}

#[test]
fn extracts_memory_id_from_nested_event_payload() {
    let value = serde_json::json!({
        "event_id": "event_1",
        "results": [
            {"memory": {"id": "mem_42", "memory": "remembered fact"}}
        ]
    });

    assert_eq!(extract_memory_id(&value).as_deref(), Some("mem_42"));
}

#[tokio::test]
async fn mem0_declares_provider_neutral_capabilities() {
    let client = Mem0Client::new(Mem0Config {
        base_url: "http://localhost:8888".to_string(),
        api_key: String::new(),
        user_id_prefix: "test_".to_string(),
        app_id: None,
        agent_id: None,
        run_id: None,
        custom_instructions: None,
        infer: false,
        conversation_infer: true,
        request_timeout: Duration::from_secs(1),
        page_size: 10,
        add_poll_timeout: Duration::from_millis(500),
    });

    let caps = client.memory_capabilities().await;

    assert_eq!(caps.abi_version, 1);
    assert!(caps.supports_store);
    assert!(caps.supports_metadata);
    assert!(caps.supports_conversation_ingest);
    assert!(caps.supports_provider_infer);
    assert!(caps.supports_search_filters);
    assert!(caps.supports_layer_filters);
    assert!(caps.supports_similarity_threshold);
    assert!(caps.async_writes);
    assert!(!caps.supports_relationship_edges);
    assert_eq!(
        caps.provider_specific["non_conversational_store"]["request_shape"],
        "single_user_message"
    );
}

#[tokio::test]
#[ignore = "requires MEM0_API_KEY and writes a temporary memory to hosted mem0"]
async fn mem0_live_store_supports_selected_memory_without_provider_infer() {
    let api_key = match std::env::var("MEM0_API_KEY") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("MEM0_API_KEY is not set; skipping live mem0 test");
            return;
        }
    };
    let client = Mem0Client::new(Mem0Config {
        base_url: std::env::var("MEM0_BASE_URL")
            .unwrap_or_else(|_| "https://api.mem0.ai".to_string()),
        api_key,
        user_id_prefix: "alia_live_test_".to_string(),
        app_id: None,
        agent_id: None,
        run_id: Some("alia-live-memory-test".to_string()),
        custom_instructions: None,
        infer: false,
        conversation_infer: false,
        request_timeout: Duration::from_secs(20),
        page_size: 10,
        add_poll_timeout: Duration::from_secs(10),
    });
    let user_id = Uuid::new_v4();

    let memory_id = client
        .store(
            user_id,
            MemoryNode {
                id: None,
                memory_layer: "preference".to_string(),
                node_type: "preference".to_string(),
                content: "The live-test user prefers concise answers.".to_string(),
                metadata: serde_json::json!({"source": "live_test"}),
                importance: 0.1,
            },
        )
        .await
        .expect("mem0 should accept selected memory with infer=false");

    let memories = client
        .list(
            user_id,
            alia_server::providers::adapter::MemoryListOpts {
                max_results: Some(10),
                layers: None,
            },
        )
        .await
        .expect("mem0 should list live-test memories");
    assert!(
        memories
            .iter()
            .any(|memory| memory.content.contains("prefers concise answers"))
    );

    client
        .delete(&memory_id)
        .await
        .expect("mem0 should delete the live-test memory");
}
