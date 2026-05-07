//! Tool format translation layer
//!
//! Converts skill definitions between the canonical DB format and
//! LLM-specific tool calling formats (Claude, OpenAI, etc.).

use serde::{Deserialize, Serialize};

use crate::entity::skill;

use super::builtin::LoadedSkill;

/// Supported LLM tool calling formats
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFormat {
    /// Anthropic Claude format: `{name, description, input_schema}`
    Claude,
    /// OpenAI format: `{type: "function", function: {name, description, parameters}}`
    OpenAI,
}

/// A skill converted to an LLM-specific tool definition
#[derive(Debug, Clone, Serialize)]
pub struct FormattedTool {
    pub format: ToolFormat,
    pub definition: serde_json::Value,
}

/// Convert a skill entity model to the specified LLM tool format.
pub fn to_tool_definition(skill: &skill::Model, format: ToolFormat) -> FormattedTool {
    let definition = match format {
        ToolFormat::Claude => to_claude(skill),
        ToolFormat::OpenAI => to_openai(skill),
    };
    FormattedTool { format, definition }
}

/// Convert a loaded (in-memory) skill to the specified LLM tool format.
pub fn loaded_to_tool_definition(skill: &LoadedSkill, format: ToolFormat) -> FormattedTool {
    let definition = match format {
        ToolFormat::Claude => loaded_to_claude(skill),
        ToolFormat::OpenAI => loaded_to_openai(skill),
    };
    FormattedTool { format, definition }
}

/// Convert all skills to tool definitions for a given format.
pub fn to_tool_definitions(skills: &[skill::Model], format: ToolFormat) -> Vec<FormattedTool> {
    skills.iter().map(|s| to_tool_definition(s, format)).collect()
}

// ========== Claude format ==========
// {
//   "name": "smart_light_control",
//   "description": "控制智能灯的开关、亮度、色温",
//   "input_schema": { "type": "object", "properties": { ... } }
// }

fn to_claude(skill: &skill::Model) -> serde_json::Value {
    serde_json::json!({
        "name": skill.name,
        "description": skill.description.as_deref().unwrap_or(&skill.display_name),
        "input_schema": ensure_object_schema(&skill.parameters),
    })
}

fn loaded_to_claude(skill: &LoadedSkill) -> serde_json::Value {
    let desc = if skill.info.description.is_empty() {
        &skill.info.display_name
    } else {
        &skill.info.description
    };
    serde_json::json!({
        "name": skill.info.name,
        "description": desc,
        "input_schema": ensure_object_schema(&skill.param_schema),
    })
}

// ========== OpenAI format ==========
// {
//   "type": "function",
//   "function": {
//     "name": "smart_light_control",
//     "description": "控制智能灯的开关、亮度、色温",
//     "parameters": { "type": "object", "properties": { ... } }
//   }
// }

fn to_openai(skill: &skill::Model) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": skill.name,
            "description": skill.description.as_deref().unwrap_or(&skill.display_name),
            "parameters": ensure_object_schema(&skill.parameters),
        }
    })
}

fn loaded_to_openai(skill: &LoadedSkill) -> serde_json::Value {
    let desc = if skill.info.description.is_empty() {
        &skill.info.display_name
    } else {
        &skill.info.description
    };
    serde_json::json!({
        "type": "function",
        "function": {
            "name": skill.info.name,
            "description": desc,
            "parameters": ensure_object_schema(&skill.param_schema),
        }
    })
}

// ========== Helpers ==========

/// Ensure the parameter schema has `"type": "object"` at the root.
/// If it's an empty `{}`, wrap it properly.
fn ensure_object_schema(schema: &serde_json::Value) -> serde_json::Value {
    if let Some(obj) = schema.as_object() {
        if obj.contains_key("type") {
            return schema.clone();
        }
        if obj.is_empty() {
            return serde_json::json!({
                "type": "object",
                "properties": {}
            });
        }
    }
    // Fallback: wrap as-is
    serde_json::json!({
        "type": "object",
        "properties": schema
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Should NOT contain execution_config
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
        // Should NOT contain execution_config
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
}
