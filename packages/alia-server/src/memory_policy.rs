use crate::providers::adapter::{MemoryNode, MemoryPatch};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensitiveMemoryMatch {
    pub category: &'static str,
    pub reason: &'static str,
}

pub fn detect_sensitive_memory(content: &str) -> Option<SensitiveMemoryMatch> {
    let text = content.trim();
    if text.is_empty() {
        return None;
    }
    let lower = text.to_ascii_lowercase();

    if looks_like_secret(text, &lower) {
        return Some(SensitiveMemoryMatch {
            category: "secret",
            reason: "secret_or_credential",
        });
    }
    if looks_like_identifier(text, &lower) {
        return Some(SensitiveMemoryMatch {
            category: "identifier",
            reason: "government_or_financial_identifier",
        });
    }
    if contains_any(
        &lower,
        &[
            "diagnosed with",
            "medical condition",
            "mental health",
            "depression",
            "anxiety disorder",
            "cancer",
            "diabetes",
            "hiv",
            "pregnant",
            "therapy",
            "medication",
            "prescription",
        ],
    ) {
        return Some(SensitiveMemoryMatch {
            category: "health",
            reason: "health_information",
        });
    }
    if contains_any(
        &lower,
        &[
            "religion",
            "religious",
            "muslim",
            "christian",
            "jewish",
            "buddhist",
            "hindu",
            "atheist",
            "political party",
            "democrat",
            "republican",
            "communist",
            "conservative",
            "liberal",
            "voted for",
            "supports the party",
        ],
    ) {
        return Some(SensitiveMemoryMatch {
            category: "belief_or_political_view",
            reason: "sensitive_belief_or_political_information",
        });
    }

    None
}

pub fn sensitive_memory_allowed_for_source(source: &str) -> bool {
    matches!(
        source.trim(),
        "user_frontend" | "user_direct_api" | "manual_user_entry"
    )
}

pub fn normalize_memory_layer(layer: &str) -> String {
    match layer.trim().to_ascii_lowercase().as_str() {
        "core" => "core".to_string(),
        "preference" | "preferences" => "preference".to_string(),
        "relationship" | "social" => "relationship".to_string(),
        "episodic" | "event" | "events" => "episodic".to_string(),
        "procedural" | "semantic" => "procedural".to_string(),
        "reflection" | "insight" => "reflection".to_string(),
        _ => "episodic".to_string(),
    }
}

pub fn normalize_memory_layers(layers: Vec<String>) -> Vec<String> {
    let mut normalized = layers
        .into_iter()
        .map(|layer| normalize_memory_layer(&layer))
        .filter(|layer| !layer.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

pub fn normalize_memory_node_type(node_type: &str) -> String {
    match node_type.trim().to_ascii_lowercase().as_str() {
        "preference" | "fact" | "relationship" | "goal" | "habit" | "event" | "insight" => {
            node_type.trim().to_ascii_lowercase()
        }
        _ => "fact".to_string(),
    }
}

pub fn memory_merge_key(layer: &str, node_type: &str, content: &str) -> Option<String> {
    let normalized = normalize_for_match(content);
    if normalized.is_empty() {
        return None;
    }

    let layer = normalize_memory_layer(layer);
    let node_type = normalize_memory_node_type(node_type);
    if layer == "preference" || node_type == "preference" {
        if contains_any(
            &normalized,
            &[
                "answer",
                "answers",
                "reply",
                "replies",
                "response",
                "responses",
                "concise",
                "detailed",
                "brief",
            ],
        ) {
            return Some("preference:response_style".to_string());
        }
        if contains_any(&normalized, &["theme", "color", "colour", "dark mode"]) {
            return Some("preference:visual_style".to_string());
        }
        if contains_any(
            &normalized,
            &["temperature", "thermostat", "air conditioner"],
        ) {
            return Some("preference:climate".to_string());
        }
    }

    Some(format!("{layer}:{node_type}:{normalized}"))
}

pub fn memory_content_equivalent(left: &str, right: &str) -> bool {
    normalize_for_match(left) == normalize_for_match(right)
}

pub fn merge_memory_patch(existing: &MemoryNode, incoming: &MemoryNode) -> MemoryPatch {
    let mut metadata = existing.metadata.clone();
    merge_json_object(
        &mut metadata,
        serde_json::json!({
            "last_merged_from": incoming.metadata,
            "merged_at": chrono::Utc::now().to_rfc3339(),
        }),
    );
    MemoryPatch {
        content: Some(incoming.content.clone()),
        metadata: Some(metadata),
        importance: Some(existing.importance.max(incoming.importance)),
        confidence: Some(existing.confidence.max(incoming.confidence)),
        salience: Some(existing.salience.max(incoming.salience)),
        ..MemoryPatch::default()
    }
}

pub fn memory_prompt_line(content: &str, max_chars: usize) -> String {
    let trimmed = content.trim();
    let mut line = trimmed.chars().take(max_chars).collect::<String>();
    if trimmed.chars().count() > max_chars {
        line.push_str("...");
    }
    line
}

fn merge_json_object(target: &mut serde_json::Value, patch: serde_json::Value) {
    if !target.is_object() {
        *target = serde_json::json!({});
    }
    let Some(target) = target.as_object_mut() else {
        return;
    };
    if let Some(patch) = patch.as_object() {
        for (key, value) in patch {
            target.insert(key.clone(), value.clone());
        }
    }
}

fn looks_like_secret(text: &str, lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "api key",
            "apikey",
            "password",
            "passwd",
            "secret",
            "token",
            "bearer ",
            "private key",
            "ssh-rsa",
            "-----begin private key-----",
        ],
    ) || text
        .split_whitespace()
        .any(|token| token.len() >= 32 && token.chars().any(char::is_numeric))
}

fn looks_like_identifier(text: &str, lower: &str) -> bool {
    if contains_any(
        lower,
        &[
            "social security number",
            "ssn",
            "passport number",
            "driver license",
            "credit card",
            "bank account",
            "routing number",
            "身份证",
            "护照",
            "银行卡",
        ],
    ) {
        return true;
    }
    let digits = text.chars().filter(|ch| ch.is_ascii_digit()).count();
    digits >= 9 && contains_any(lower, &["id", "number", "card", "account", "ssn"])
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn normalize_for_match(text: &str) -> String {
    let mut normalized = text
        .to_ascii_lowercase()
        .replace("the user ", "")
        .replace("user ", "")
        .replace(" no longer ", " ")
        .replace(" does not ", " ")
        .replace(" doesn't ", " ")
        .replace(" do not ", " ")
        .replace(" don't ", " ");
    normalized.retain(|ch| ch.is_ascii_alphanumeric() || ch.is_ascii_whitespace());
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sensitive_memory_categories() {
        assert_eq!(
            detect_sensitive_memory("The user API key is sk-live-12345678901234567890abcdef")
                .unwrap()
                .category,
            "secret"
        );
        assert_eq!(
            detect_sensitive_memory("The user's social security number is 123-45-6789")
                .unwrap()
                .category,
            "identifier"
        );
        assert_eq!(
            detect_sensitive_memory("The user was diagnosed with diabetes")
                .unwrap()
                .category,
            "health"
        );
    }

    #[test]
    fn response_style_preferences_share_merge_key() {
        assert_eq!(
            memory_merge_key(
                "preference",
                "preference",
                "The user prefers concise answers."
            ),
            memory_merge_key(
                "preference",
                "preference",
                "The user prefers detailed replies."
            )
        );
    }
}
