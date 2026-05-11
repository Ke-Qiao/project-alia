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
//! tools = ["device_control", "sensor_read"]
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
    /// Tool names to make available to the LLM during this skill's execution.
    #[serde(default)]
    pub tools: Vec<String>,
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
        toml::Value::Array(arr) => serde_json::Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => {
            let map: serde_json::Map<String, serde_json::Value> = table
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
    }
}
