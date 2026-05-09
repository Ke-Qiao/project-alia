use alia_server::entity::skill;
use alia_server::providers::skills::builtin::{builtin_skill_seeds, BuiltinSkills, LoadedSkill};
use alia_server::providers::skills::markdown::{parse_skill_file, toml_to_json};
use alia_server::providers::skills::tool_format::{to_tool_definition, ToolFormat};
use alia_server::providers::SkillInfo;

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
        execution_config: None,
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
    assert_eq!(
        parsed.prompt_template.as_deref(),
        Some("这是 prompt template。")
    );
}

#[test]
fn test_parse_skill_file_with_parameters() {
    let content = r#"+++
name = "weather"
display_name = "天气查询"
category = "information"
intent_patterns = ["天气", "weather"]
cacheable = true

[parameters]
type = "object"

[parameters.properties.location]
type = "string"
description = "地点"

[execution_config]
type = "function"

[execution_config.function]
name = "weather_query"
+++

查询 {location} 的天气。
"#;
    let parsed = parse_skill_file(content).unwrap();
    assert_eq!(parsed.frontmatter.name, "weather");
    assert!(parsed.frontmatter.cacheable);
    assert!(parsed.frontmatter.parameters.is_some());
    assert!(parsed.frontmatter.execution_config.is_some());

    let params_json = toml_to_json(parsed.frontmatter.parameters.as_ref().unwrap());
    assert_eq!(params_json["type"], "object");
    assert_eq!(params_json["properties"]["location"]["type"], "string");
}

#[test]
fn test_parse_skill_file_no_body() {
    let content = r#"+++
name = "no_body"
display_name = "No Body"
category = "test"
intent_patterns = ["test"]
+++"#;
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

// ── tool format ────────────────────────────────────────────────────────

fn sample_skill() -> skill::Model {
    skill::Model {
        id: uuid::Uuid::new_v4(),
        name: "smart_light_control".into(),
        display_name: "智能灯光控制".into(),
        description: Some("控制智能灯的开关、亮度、色温".into()),
        category: "smart_home".into(),
        intent_patterns: serde_json::json!(["灯", "开灯", "关灯"]),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["on", "off"]},
                "device_name": {"type": "string"}
            },
            "required": ["action"]
        }),
        execution_config: Some(serde_json::json!({
            "plugin_name": "zigbee-bridge",
            "action": "light_control"
        })),
        prompt_template: None,
        cacheable: false,
        enabled: true,
        priority: 10,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

#[test]
fn test_claude_format() {
    let skill = sample_skill();
    let result = to_tool_definition(&skill, ToolFormat::Claude);

    let def = &result.definition;
    assert_eq!(def["name"], "smart_light_control");
    assert_eq!(def["description"], "控制智能灯的开关、亮度、色温");
    assert_eq!(def["input_schema"]["type"], "object");
    assert!(def["input_schema"]["properties"]["action"].is_object());
    assert!(def.get("execution_config").is_none());
}

#[test]
fn test_openai_format() {
    let skill = sample_skill();
    let result = to_tool_definition(&skill, ToolFormat::OpenAI);

    let def = &result.definition;
    assert_eq!(def["type"], "function");
    assert_eq!(def["function"]["name"], "smart_light_control");
    assert_eq!(def["function"]["description"], "控制智能灯的开关、亮度、色温");
    assert_eq!(def["function"]["parameters"]["type"], "object");
    assert!(def["function"].get("execution_config").is_none());
}

#[test]
fn test_empty_params_normalized() {
    let mut skill = sample_skill();
    skill.parameters = serde_json::json!({});

    let claude = to_tool_definition(&skill, ToolFormat::Claude);
    assert_eq!(claude.definition["input_schema"]["type"], "object");

    let openai = to_tool_definition(&skill, ToolFormat::OpenAI);
    assert_eq!(openai.definition["function"]["parameters"]["type"], "object");
}
