use alia_server::providers::SkillInfo;
use alia_server::providers::skills::builtin::{BuiltinSkills, LoadedSkill, builtin_skill_seeds};
use alia_server::providers::skills::markdown::parse_skill_file;

// ── builtin seeds ──────────────────────────────────────────────────────

#[test]
fn test_seed_data() {
    let seeds = builtin_skill_seeds();
    assert_eq!(seeds.len(), 8);

    let names: Vec<String> = seeds.iter().map(|s| s.name.clone().unwrap()).collect();
    assert!(names.contains(&"smart_light_control".to_string()));
    assert!(names.contains(&"smart_ac_control".to_string()));
    assert!(names.contains(&"smart_appliance".to_string()));
    assert!(names.contains(&"timer_alarm".to_string()));
    assert!(names.contains(&"weather_query".to_string()));
    assert!(names.contains(&"device_status".to_string()));
    assert!(names.contains(&"memory_query".to_string()));
    assert!(names.contains(&"rule_management".to_string()));
}

#[test]
fn test_seed_data_uses_tool_name_arrays() {
    let seeds = builtin_skill_seeds();
    for seed in &seeds {
        if let Some(ref config) = seed.execution_config.clone().unwrap() {
            // All seed data should use the new JSON array format
            assert!(
                config.is_array(),
                "seed skill '{}' execution_config should be a JSON array of tool names, got: {}",
                seed.name.clone().unwrap(),
                config
            );
        }
    }
}

#[test]
fn test_match_scoring() {
    let skill = LoadedSkill {
        info: SkillInfo {
            name: "test".into(),
            display_name: "Test".into(),
            description: "Test skill".into(),
            category: "test".into(),
        },
        keywords: vec!["灯".into(), "开灯".into(), "灯光".into()],
        param_schema: serde_json::json!({}),
        tools: vec!["device_control".to_string()],
        prompt_template: None,
        cacheable: false,
        priority: 0,
    };

    let score = BuiltinSkills::calculate_match_score("帮我开灯", &skill);
    assert!(score > 0.0, "should match '开灯'");

    let score = BuiltinSkills::calculate_match_score("今天心情不错", &skill);
    assert_eq!(score, 0.0, "should not match irrelevant text");
}

// ── markdown parser ────────────────────────────────────────────────────

#[test]
fn test_parse_skill_file_basic() {
    let content = r#"+++
name = "test_skill"
display_name = "测试技能"
category = "utility"
intent_patterns = ["测试", "test"]
priority = 5
+++

这是 prompt template。
"#;
    let parsed = parse_skill_file(content).unwrap();
    assert_eq!(parsed.frontmatter.name, "test_skill");
    assert_eq!(parsed.frontmatter.display_name, "测试技能");
    assert_eq!(parsed.frontmatter.category, "utility");
    assert_eq!(parsed.frontmatter.intent_patterns, vec!["测试", "test"]);
    assert_eq!(parsed.frontmatter.priority, 5);
    assert!(parsed.frontmatter.enabled);
    assert!(!parsed.frontmatter.cacheable);
    assert!(parsed.frontmatter.tools.is_empty());
    assert_eq!(
        parsed.prompt_template.as_deref(),
        Some("这是 prompt template。")
    );
}

#[test]
fn test_parse_skill_file_with_tools() {
    let content = r#"+++
name = "weather"
display_name = "天气查询"
category = "information"
intent_patterns = ["天气", "weather"]
tools = ["sensor_read", "sensor_list"]
cacheable = true

[parameters]
type = "object"

[parameters.properties.location]
type = "string"
description = "地点"
+++

查询 {location} 的天气。
"#;
    let parsed = parse_skill_file(content).unwrap();
    assert_eq!(parsed.frontmatter.name, "weather");
    assert!(parsed.frontmatter.cacheable);
    assert_eq!(parsed.frontmatter.tools, vec!["sensor_read", "sensor_list"]);
    assert!(parsed.frontmatter.parameters.is_some());
}

#[test]
fn test_parse_skill_file_no_body() {
    let content = r#"+++
name = "no_body"
display_name = "No Body"
category = "test"
intent_patterns = ["test"]
+++
"#;
    let parsed = parse_skill_file(content).unwrap();
    assert!(parsed.prompt_template.is_none());
}

#[test]
fn test_parse_skill_file_missing_delimiter() {
    let result = parse_skill_file("no delimiter");
    assert!(result.is_err());
}

#[test]
fn test_parse_skill_file_missing_closing() {
    let result = parse_skill_file("+++\nname = \"x\"\n");
    assert!(result.is_err());
}

#[test]
fn test_toml_to_json() {
    use alia_server::providers::skills::markdown::toml_to_json;

    let toml_val: toml::Value = toml::from_str(
        r#"
            type = "object"
            [properties.name]
            type = "string"
            description = "test"
            "#,
    )
    .unwrap();

    let json = toml_to_json(&toml_val);
    assert_eq!(json["type"], "object");
    assert_eq!(json["properties"]["name"]["type"], "string");
}
