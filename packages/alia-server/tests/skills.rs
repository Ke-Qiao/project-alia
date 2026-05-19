use alia_server::providers::SkillInfo;
use alia_server::providers::skills::builtin::{
    BuiltinSkills, LoadedSkill, builtin_skill_seeds, builtin_skill_tool_bindings,
};
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
fn test_seed_data_uses_skill_tool_bindings() {
    let bindings = builtin_skill_tool_bindings();
    assert_eq!(bindings.len(), 8);
    assert!(bindings.iter().any(|(skill, tools)| {
        *skill == "device_status" && tools == &["sensor_read", "sensor_list"]
    }));
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
        keywords: vec!["light".into(), "turn on light".into(), "lighting".into()],
        tools: vec!["device_control".to_string()],
        prompt_template: None,
        cacheable: false,
        priority: 0,
    };

    let score = BuiltinSkills::calculate_match_score("please turn on the light", &skill);
    assert!(score > 0.0, "should match light control");

    let score = BuiltinSkills::calculate_match_score("I feel good today", &skill);
    assert_eq!(score, 0.0, "should not match irrelevant text");
}

// ── markdown parser ────────────────────────────────────────────────────

#[test]
fn test_parse_skill_file_basic() {
    let content = r#"+++
name = "test_skill"
display_name = "Test Skill"
category = "utility"
intent_patterns = ["test", "demo"]
priority = 5
+++

This is the prompt template.
"#;
    let parsed = parse_skill_file(content).unwrap();
    assert_eq!(parsed.frontmatter.name, "test_skill");
    assert_eq!(parsed.frontmatter.display_name, "Test Skill");
    assert_eq!(parsed.frontmatter.category, "utility");
    assert_eq!(parsed.frontmatter.intent_patterns, vec!["test", "demo"]);
    assert_eq!(parsed.frontmatter.priority, 5);
    assert!(parsed.frontmatter.enabled);
    assert!(!parsed.frontmatter.cacheable);
    assert!(parsed.frontmatter.tools.is_empty());
    assert_eq!(
        parsed.prompt_template.as_deref(),
        Some("This is the prompt template.")
    );
}

#[test]
fn test_parse_skill_file_with_tools() {
    let content = r#"+++
name = "weather"
display_name = "Weather Lookup"
category = "information"
intent_patterns = ["weather", "forecast"]
tools = ["sensor_read", "sensor_list"]
cacheable = true
+++

Check the weather for {location}.
"#;
    let parsed = parse_skill_file(content).unwrap();
    assert_eq!(parsed.frontmatter.name, "weather");
    assert!(parsed.frontmatter.cacheable);
    assert_eq!(parsed.frontmatter.tools, vec!["sensor_read", "sensor_list"]);
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
