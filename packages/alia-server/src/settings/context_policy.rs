//! Runtime settings are a projection of this policy, never its authority.
use std::collections::BTreeMap;

use anyhow::Context;
use serde_json::Value;

use crate::modules::memory::context::ContextPolicy;

/// Map only policy-bearing settings. Other provider settings retain their own
/// lifecycle; a PATCH must not replace unrelated policy with stale RAM values.
const FIELDS: &[(&str, &str)] = &[
    ("core.policies.memory.enabled", "memory_enabled"),
    ("core.policies.memory.capture_mode", "capture_mode"),
    (
        "core.policies.memory.sensitive_filter_enabled",
        "sensitive_filter_enabled",
    ),
    ("core.policies.memory.paused_until", "paused_until"),
    (
        "core.policies.memory.active_embedding_profile",
        "active_embedding_profile",
    ),
    ("core.policies.memory.graph_depth", "graph_depth"),
    (
        "core.policies.memory.consolidation_enabled",
        "consolidation_enabled",
    ),
    ("core.policies.memory.retrieval_layers", "retrieval_layers"),
    (
        "core.policies.memory.max_injected_memory_chars",
        "max_memory_chars",
    ),
    (
        "core.policies.memory.max_retrieval_results",
        "max_memory_results",
    ),
    (
        "core.policies.memory.similarity_threshold",
        "similarity_threshold",
    ),
    (
        "core.policies.memory.capture_min_confidence",
        "capture_min_confidence",
    ),
    (
        "core.policies.memory.capture_min_importance",
        "capture_min_importance",
    ),
    (
        "core.policies.conversation.max_context_messages",
        "max_history_messages",
    ),
    ("core.policies.cache.enabled", "cache_enabled"),
    (
        "core.policies.cache.exact_match_enabled",
        "cache_exact_enabled",
    ),
    (
        "core.policies.cache.semantic_match_enabled",
        "cache_semantic_enabled",
    ),
    (
        "core.policies.cache.semantic_threshold",
        "cache_semantic_threshold",
    ),
    (
        "core.policies.cache.default_ttl_seconds",
        "cache_default_ttl_seconds",
    ),
    (
        "core.policies.cache.skill_cache_ttl_seconds",
        "cache_skill_ttl_seconds",
    ),
];

pub fn owns(key: &str) -> bool {
    FIELDS.iter().any(|(setting, _)| *setting == key)
}

pub fn patched(
    current: &ContextPolicy,
    updates: &BTreeMap<String, Value>,
) -> anyhow::Result<ContextPolicy> {
    let mut value = serde_json::to_value(current)?;
    for (setting, field) in FIELDS {
        if let Some(next) = updates.get(*setting) {
            value[*field] =
                if *field == "paused_until" && next.as_str().is_some_and(|s| s.trim().is_empty()) {
                    Value::Null
                } else {
                    next.clone()
                };
        }
    }
    let next: ContextPolicy =
        serde_json::from_value(value).context("invalid context policy setting type")?;
    next.validate()?;
    Ok(next)
}

pub fn projection(policy: &ContextPolicy) -> BTreeMap<String, Value> {
    let value = serde_json::to_value(policy).expect("validated context policy is serializable");
    FIELDS
        .iter()
        .map(|(setting, field)| ((*setting).to_owned(), value[*field].clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConversationSection, MemorySection};

    #[test]
    fn patch_preserves_unmentioned_policy_and_rejects_invalid_types() {
        let current =
            ContextPolicy::from_config(&MemorySection::default(), &ConversationSection::default())
                .unwrap();
        let changes = BTreeMap::from([("core.policies.memory.enabled".into(), Value::Bool(false))]);
        let next = patched(&current, &changes).unwrap();
        assert!(!next.memory_enabled);
        assert_eq!(next.capture_mode, current.capture_mode);
        assert_eq!(next.cache_enabled, current.cache_enabled);
        assert!(
            patched(
                &current,
                &BTreeMap::from([(
                    "core.policies.memory.enabled".into(),
                    Value::String("false".into())
                )])
            )
            .is_err()
        );
        assert!(
            patched(
                &current,
                &BTreeMap::from([(
                    "core.policies.memory.capture_mode".into(),
                    Value::String("misspelled".into())
                )])
            )
            .is_err()
        );
    }
}
