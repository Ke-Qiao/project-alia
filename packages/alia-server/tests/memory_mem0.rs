use alia_server::providers::memory::mem0::{extract_memory_id, parse_memory_nodes};

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
