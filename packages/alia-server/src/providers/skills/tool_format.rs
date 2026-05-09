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
    skills
        .iter()
        .map(|s| to_tool_definition(s, format))
        .collect()
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
