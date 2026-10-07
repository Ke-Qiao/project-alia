use super::prompt::preview_for_prompt;
use super::services::InteractionServices;
use crate::config::{CachePolicyResourceSection, CacheSection};
use crate::providers::adapter::{
    CACHE_METADATA_SCHEMA, CachedResponse, DEFAULT_CACHE_SCOPE, SemanticCacheCandidate,
};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CacheWrite {
    pub scope: String,
    pub metadata: serde_json::Value,
    pub ttl_seconds: u64,
    pub store_embedding: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CacheContext {
    pub memory_context_used: bool,
    pub used_tools: bool,
    pub skill_cacheable: Option<bool>,
    pub response_was_fallback: bool,
    pub route: &'static str,
}

pub fn cached_response_is_safe_direct(cached: &CachedResponse) -> bool {
    let metadata = &cached.metadata;
    let policy = metadata
        .get("policy")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();

    metadata.get("schema").and_then(serde_json::Value::as_str) == Some(CACHE_METADATA_SCHEMA)
        && matches!(policy, "private_exact" | "shared_exact")
        && metadata
            .get("safe_direct_return")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && metadata
            .get("memory_independent")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && metadata
            .get("tool_free")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && metadata
            .get("dynamic_free")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        && metadata.get("status").and_then(serde_json::Value::as_str) == Some(CACHE_STATUS_ACTIVE)
}

pub fn cache_write_for_response(
    cache_config: &CacheSection,
    cache_policy: &CachePolicyResourceSection,
    query_text: &str,
    response_text: &str,
    context: CacheContext,
) -> Option<CacheWrite> {
    if !cache_config.enabled || !cache_config.exact_match_enabled {
        return None;
    }
    if context.response_was_fallback
        || context.used_tools
        || context.memory_context_used
        || response_text.trim().is_empty()
        || matches!(context.route, "router_cache" | "major_llm_escalated")
    {
        return None;
    }
    if matches!(context.skill_cacheable, Some(false)) {
        return None;
    }
    if query_matches_keywords(query_text, &cache_policy.dynamic_query_keywords)
        || query_matches_keywords(query_text, &cache_policy.memory_query_keywords)
        || query_matches_keywords(query_text, &cache_policy.action_query_keywords)
    {
        return None;
    }

    let ttl_seconds = if context.skill_cacheable == Some(true) {
        cache_config.skill_cache_ttl_seconds
    } else {
        cache_config.default_ttl_seconds
    };
    if ttl_seconds == 0 {
        return None;
    }

    Some(CacheWrite {
        scope: DEFAULT_CACHE_SCOPE.into(),
        metadata: serde_json::json!({
            "schema": CACHE_METADATA_SCHEMA,
            "policy": "private_exact",
            "status": CACHE_STATUS_PENDING_INSIGHTS_REVIEW,
            "safe_direct_return": false,
            "memory_independent": true,
            "tool_free": true,
            "dynamic_free": true,
            "route": context.route,
            "language": guess_text_language(query_text),
            "created_by": "pipeline",
            "review_required_by": "insights",
        }),
        ttl_seconds,
        store_embedding: cache_config.semantic_match_enabled,
    })
}

pub(super) async fn semantic_cache_candidates(
    modules: &InteractionServices,
    user_id: Uuid,
    query_text: &str,
    debug_output: bool,
) -> Vec<SemanticCacheCandidate> {
    if modules.embedding.name() == "noop_embedding" {
        return Vec::new();
    }

    let profiled = match modules
        .embedding
        .embed_profiled(&[query_text.to_string()])
        .await
    {
        Ok(Some(profiled)) if profiled.vectors.len() == 1 => profiled,
        Ok(_) => return Vec::new(),
        Err(error) => {
            tracing::debug!(?error, "semantic cache embedding lookup failed");
            return Vec::new();
        }
    };
    let candidates = match modules
        .cache
        .get_semantic_candidates(
            user_id,
            DEFAULT_CACHE_SCOPE,
            &profiled.profile,
            &profiled.vectors[0],
            3,
        )
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::debug!(?error, "semantic cache candidate lookup failed");
            Vec::new()
        }
    };
    if candidates.is_empty() {
        return Vec::new();
    }

    if debug_output {
        tracing::info!(
            target: "alia_server::debug_output",
            component = "cache",
            event = "semantic_candidates",
            candidates = candidates.len(),
            "component event"
        );
    }

    candidates
}

pub(super) fn semantic_cache_reference_prompt(
    candidates: &[SemanticCacheCandidate],
    debug_output: bool,
) -> Option<(String, usize)> {
    let candidates = candidates
        .iter()
        .filter(|candidate| semantic_candidate_is_safe_reference(candidate))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return None;
    }

    if debug_output {
        tracing::info!(
            target: "alia_server::debug_output",
            component = "cache",
            event = "semantic_references",
            candidates = candidates.len(),
            "component event"
        );
    }

    let references = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            format!(
                "{}. similarity={:.3}: {}",
                index + 1,
                candidate.similarity,
                preview_for_prompt(&candidate.cached.response_text, 360)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let candidate_count = candidates.len();
    Some((
        format!(
            "Internal cache references. These are semantically similar previous answers, not authoritative facts. Reuse or adapt them only if they clearly answer the current user request; otherwise ignore them. Do not mention the cache.\n\n{}",
            references
        ),
        candidate_count,
    ))
}

pub(super) fn semantic_candidate_is_safe_direct(candidate: &SemanticCacheCandidate) -> bool {
    semantic_candidate_is_safe_reference(candidate)
}

pub(super) fn semantic_candidate_is_safe_reference(candidate: &SemanticCacheCandidate) -> bool {
    cached_response_is_safe_direct(&candidate.cached)
}

pub(super) fn query_matches_keywords(text: &str, keywords: &[String]) -> bool {
    let lower = text.to_lowercase();
    keywords
        .iter()
        .map(|keyword| keyword.trim().to_lowercase())
        .filter(|keyword| !keyword.is_empty())
        .any(|keyword| lower.contains(&keyword))
}

pub(super) fn guess_text_language(text: &str) -> &'static str {
    if text.chars().any(|ch| {
        ('\u{4e00}'..='\u{9fff}').contains(&ch) || ('\u{3400}'..='\u{4dbf}').contains(&ch)
    }) {
        "zh"
    } else {
        "unknown"
    }
}

const CACHE_STATUS_ACTIVE: &str = "active";
const CACHE_STATUS_PENDING_INSIGHTS_REVIEW: &str = "pending_insights_review";
