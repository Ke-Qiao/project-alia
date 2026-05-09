//! Markdown skill file loader
//!
//! Loads skill definitions from `.md` files with TOML frontmatter (`+++`).
//! File body (after the closing `+++`) becomes the `prompt_template`.
//!
//! ## Format
//!
//! ```markdown
//! +++
//! name = "my_skill"
//! display_name = "我的技能"
//! category = "utility"
//! intent_patterns = ["关键词1", "关键词2"]
//! priority = 5
//!
//! [parameters.properties.location]
//! type = "string"
//! description = "位置"
//! +++
//!
//! Prompt template content here...
//! ```

use std::path::Path;

use serde::Deserialize;

/// Parsed TOML frontmatter from a skill markdown file.
#[derive(Debug, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub category: String,
    pub intent_patterns: Vec<String>,
    #[serde(default)]
    pub parameters: Option<toml::Value>,
    #[serde(default)]
    pub execution_config: Option<toml::Value>,
    #[serde(default)]
    pub cacheable: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i32,
}

fn default_enabled() -> bool {
    true
}

/// A fully parsed skill file: frontmatter + body.
pub struct ParsedSkillFile {
    pub frontmatter: SkillFrontmatter,
    pub prompt_template: Option<String>,
}

/// Parse a single markdown skill file with TOML frontmatter.
///
/// The file must start with `+++`, followed by TOML content, then a closing `+++`.
/// Everything after the closing delimiter is the prompt template.
pub fn parse_skill_file(content: &str) -> anyhow::Result<ParsedSkillFile> {
    let content = content.trim();

    if !content.starts_with("+++") {
        anyhow::bail!("skill file must start with +++ TOML frontmatter delimiter");
    }

    let after_first = &content[3..];
    let end_pos = after_first
        .find("+++")
        .ok_or_else(|| anyhow::anyhow!("missing closing +++ delimiter"))?;

    let toml_str = &after_first[..end_pos];
    let body = after_first[end_pos + 3..].trim();

    let frontmatter: SkillFrontmatter = toml::from_str(toml_str)?;

    let prompt_template = if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    };

    Ok(ParsedSkillFile {
        frontmatter,
        prompt_template,
    })
}

/// Load all `.md` skill files from a directory.
///
/// Files that fail to parse are logged and skipped (non-fatal).
pub async fn load_skill_files(dir: &Path) -> anyhow::Result<Vec<ParsedSkillFile>> {
    let mut skills = Vec::new();

    if !dir.exists() {
        tracing::debug!(path = %dir.display(), "skills directory does not exist, skipping");
        return Ok(skills);
    }

    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        match tokio::fs::read_to_string(&path).await {
            Ok(content) => match parse_skill_file(&content) {
                Ok(parsed) => {
                    tracing::debug!(
                        file = %path.display(),
                        skill = %parsed.frontmatter.name,
                        "loaded skill from markdown"
                    );
                    skills.push(parsed);
                }
                Err(e) => {
                    tracing::warn!(
                        file = %path.display(),
                        error = %e,
                        "failed to parse skill file, skipping"
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    file = %path.display(),
                    error = %e,
                    "failed to read skill file, skipping"
                );
            }
        }
    }

    tracing::info!(count = skills.len(), dir = %dir.display(), "loaded markdown skill files");
    Ok(skills)
}

/// Convert a `toml::Value` to `serde_json::Value`.
pub fn toml_to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::json!(i),
        toml::Value::Float(f) => serde_json::json!(f),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(toml_to_json).collect())
        }
        toml::Value::Table(table) => {
            let map: serde_json::Map<String, serde_json::Value> = table
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
