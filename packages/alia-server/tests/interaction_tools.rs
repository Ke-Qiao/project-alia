use alia_server::modules::interaction::tools::{
    rank_and_limit_tools, rank_and_limit_tools_with_index,
};
use alia_server::providers::adapter::{ToolDefinition, ToolRoutingIndexEntry};

fn tool(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        function: rig_core::completion::ToolDefinition::new(
            rig_core::message::ToolName::new(name.to_string()).unwrap(),
            description.to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "timezone": {"type": "string"}
                }
            }),
        ),
        routing_hints: Vec::new(),
        source: "builtin".to_string(),
        require_confirmation: false,
        required_role: None,
    }
}

fn routing_entry(name: &str, keywords: &[&str], routing_text: &str) -> ToolRoutingIndexEntry {
    ToolRoutingIndexEntry {
        name: name.to_string(),
        routing_text: routing_text.to_string(),
        routing_keywords: keywords.iter().map(|keyword| keyword.to_string()).collect(),
        routing_embedding: None,
    }
}

#[test]
fn routes_global_tools_by_query_relevance() {
    let selected = rank_and_limit_tools(
        vec![
            tool("sensor_read", "Read a sensor value"),
            tool("get_current_time", "Get the current date and time"),
            tool("fetch", "Fetch a public HTTP URL"),
        ],
        "what time is it now?",
        2,
        &[],
        "cli",
    );

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].name, "get_current_time");
}

#[test]
fn avoids_global_tool_injection_when_no_tool_matches() {
    let selected = rank_and_limit_tools(
        vec![
            tool("sensor_read", "Read a sensor value"),
            tool("fetch", "Fetch a public HTTP URL"),
        ],
        "tell me a bedtime story",
        8,
        &[],
        "cli",
    );

    assert!(selected.is_empty());
}

#[test]
fn preferred_tools_are_kept_for_skill_context_when_nothing_else_matches() {
    let selected = rank_and_limit_tools(
        vec![
            tool("skill_tool_a", "A domain tool"),
            tool("skill_tool_b", "Another domain tool"),
            tool("skill_tool_c", "Third domain tool"),
        ],
        "unrelated input",
        2,
        &["skill_tool_a", "skill_tool_b", "skill_tool_c"],
        "cli",
    );

    assert_eq!(selected.len(), 2);
    assert_eq!(selected[0].name, "skill_tool_a");
    assert_eq!(selected[1].name, "skill_tool_b");
}

#[test]
fn preferred_skill_tools_do_not_hide_globally_matching_tools() {
    let mut home_assistant_control = tool(
        "home_assistant_light_control",
        "Turn a Home Assistant light on or off by entity_id or friendly name",
    );
    home_assistant_control.routing_hints = vec![
        "home assistant".to_string(),
        "灯".to_string(),
        "灯光".to_string(),
        "开灯".to_string(),
        "打开".to_string(),
        "light".to_string(),
    ];

    let selected = rank_and_limit_tools_with_index(
        vec![
            tool("sensor_read", "Read a sensor value"),
            tool("sensor_list", "List local sensors"),
            home_assistant_control,
        ],
        vec![routing_entry(
            "home_assistant_light_control",
            &["home assistant", "灯", "灯光", "开灯", "打开", "light"],
            "Control Home Assistant lights. 灯 灯光 开灯 打开 light.",
        )],
        "那帮我把这盏灯打开",
        2,
        &["sensor_read", "sensor_list"],
        "cli",
    );

    assert!(
        selected
            .iter()
            .any(|tool| tool.name == "home_assistant_light_control"),
        "matching global tools should still be selected even when the active skill prefers sensor tools"
    );
}

#[test]
fn routes_preference_statements_to_memory_store() {
    let selected = rank_and_limit_tools_with_index(
        vec![
            tool("memory_search", "Search the user's memory"),
            tool("memory_store", "Store a piece of information to memory"),
            tool("fetch", "Fetch a public HTTP URL"),
        ],
        vec![routing_entry(
            "memory_store",
            &["likes", "favorite", "preference", "remember"],
            "Save explicit user preferences and things the user asks the assistant to remember.",
        )],
        "KeQiao really likes Japanese musician tsukuyomi",
        8,
        &[],
        "cli",
    );

    assert!(selected.iter().any(|tool| tool.name == "memory_store"));
}

#[test]
fn routes_embedded_mic_volume_to_embedded_volume_tool() {
    for query in [
        "set microphone volume to 2",
        "把麦克风的声音调整到5",
        "把麥克風的聲音調整到5",
        "把麦克風的聲音調整到5",
    ] {
        let mut volume_tool = tool(
            "embedded_volume_control",
            "Get or set microphone and speaker volume for an alia-embedded channel",
        );
        volume_tool.routing_hints = vec![
            "microphone".to_string(),
            "speaker".to_string(),
            "volume".to_string(),
            "麦克风".to_string(),
            "麦克風".to_string(),
            "麥克風".to_string(),
            "声音".to_string(),
            "聲音".to_string(),
        ];
        let selected = rank_and_limit_tools_with_index(
            vec![tool("sensor_read", "Read a sensor value"), volume_tool],
            vec![routing_entry(
                "embedded_volume_control",
                &[
                    "microphone",
                    "speaker",
                    "volume",
                    "麦克风",
                    "麦克風",
                    "麥克風",
                    "声音",
                    "聲音",
                    "扬声器",
                    "揚聲器",
                ],
                "Control embedded microphone capture volume and speaker playback volume. 调整 麦克风 麥克風 声音 聲音 音量.",
            )],
            query,
            1,
            &[],
            "pi-living-room",
        );

        assert_eq!(
            selected.len(),
            1,
            "query should select volume tool: {query}"
        );
        assert_eq!(selected[0].name, "embedded_volume_control");
    }
}

#[test]
fn routes_search_language_to_fetch_candidate() {
    let selected = rank_and_limit_tools_with_index(
        vec![
            tool("sensor_read", "Read a sensor value"),
            tool("fetch", "Fetch a public HTTP URL"),
        ],
        vec![routing_entry(
            "fetch",
            &["search", "latest", "lookup", "web"],
            "Fetch or search public web resources for current/latest information.",
        )],
        "search for the latest sherpa onnx version",
        8,
        &[],
        "cli",
    );

    assert!(selected.iter().any(|tool| tool.name == "fetch"));
}

#[test]
fn db_routing_keywords_can_match_without_definition_text_overlap() {
    let mut plugin_tool = tool("music_lookup", "Catalog operation");
    plugin_tool.source = "music-plugin".to_string();
    let selected = rank_and_limit_tools_with_index(
        vec![tool("alpha_tool", "Alpha implementation"), plugin_tool],
        vec![routing_entry(
            "music_lookup",
            &["tsukuyomi", "musician", "song"],
            "Find music-related facts from a plugin catalog.",
        )],
        "tell me about tsukuyomi",
        4,
        &[],
        "cli",
    );

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].name, "music_lookup");
}

#[test]
fn url_and_arithmetic_structural_rules_still_work_without_db_keywords() {
    let url_selected = rank_and_limit_tools(
        vec![
            tool("fetch", "Public HTTP retrieval"),
            tool("sensor_read", "Read a sensor value"),
        ],
        "open https://example.com",
        2,
        &[],
        "cli",
    );
    let math_selected = rank_and_limit_tools(
        vec![
            tool("calculate", "Evaluate expressions"),
            tool("sensor_read", "Read a sensor value"),
        ],
        "what is (2 + 3) * 4?",
        2,
        &[],
        "cli",
    );

    assert_eq!(url_selected[0].name, "fetch");
    assert_eq!(math_selected[0].name, "calculate");
}
