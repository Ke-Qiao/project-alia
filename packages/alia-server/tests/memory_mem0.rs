use std::time::Duration;

use alia_server::providers::memory::external::ExternalMemoryClient;
use alia_server::providers::memory::mem0::{
    Mem0Client, Mem0Config, extract_memory_id, parse_memory_nodes,
};

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
}
