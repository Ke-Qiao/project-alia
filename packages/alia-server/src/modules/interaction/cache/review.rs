//! Review pending cache responses under their original authority and revision.

use super::{CACHE_METADATA_SCHEMA, CacheCas, CacheReview, CacheStore, CacheWork, CachedResponse};
use crate::providers::adapter::{ChatMessage, EmbeddingAdapter};
use crate::providers::llm::LlmService;
use crate::providers::llm::response::parse_llm_json_object;
use chrono::Utc;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Default)]
pub(crate) struct CacheReviewRuntime {
    pub(crate) cache: Option<Arc<dyn CacheStore>>,
    pub(crate) embedding: Option<Arc<dyn EmbeddingAdapter>>,
}

type PendingCacheEntry = CachedResponse;

#[derive(Debug, Clone, Default)]
pub(crate) struct CacheReviewStats {
    pub(crate) approved: u32,
    pub(crate) deleted: u32,
    pub(crate) pending: u32,
    pub(crate) model_used: String,
}

#[derive(Debug, Deserialize)]
struct CacheReviewDecision {
    decision: String,
    #[serde(default)]
    refined_response: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    safe_direct_return: Option<bool>,
    #[serde(default)]
    memory_independent: Option<bool>,
    #[serde(default)]
    tool_free: Option<bool>,
    #[serde(default)]
    dynamic_free: Option<bool>,
}

pub(crate) async fn review_pending_cache_entries(
    runtime: &CacheReviewRuntime,
    llm: Option<Arc<LlmService>>,
    user_id: Uuid,
    limit: usize,
) -> anyhow::Result<CacheReviewStats> {
    let Some(cache) = &runtime.cache else {
        return Ok(CacheReviewStats::default());
    };
    cache.cleanup_expired(256).await?;
    let entries = cache.load_pending(user_id, None, limit).await?;
    let mut stats = CacheReviewStats {
        pending: entries.len() as u32,
        model_used: "none".to_string(),
        ..CacheReviewStats::default()
    };
    let Some(llm) = llm else {
        return Ok(stats);
    };
    for entry in entries {
        // A batch can wait behind earlier model requests. Its initial read
        // is not continuing permission to send private text to a provider.
        if !cache.authorize_work(&entry, CacheWork::Review).await? {
            continue;
        }
        stats.model_used = llm.name().to_string();
        let decision = match review_cache_entry_with_llm(llm.as_ref(), &entry).await {
            Ok(decision) => decision,
            Err(error) => {
                tracing::warn!(cache_entry_id = %entry.version.cache_id, error = ?error,
                    "cache review failed; no decision was committed");
                continue;
            }
        };
        let action = decision.decision.trim().to_ascii_lowercase();
        let review = if action == "approve" && cache_review_approves_direct_return(&decision) {
            let response_text = decision
                .refined_response
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .unwrap_or(entry.response_text.trim())
                .to_string();
            let mut metadata = entry.metadata.clone();
            metadata["schema"] = serde_json::json!(CACHE_METADATA_SCHEMA);
            metadata["policy"] = serde_json::json!("private_exact");
            for flag in [
                "safe_direct_return",
                "memory_independent",
                "tool_free",
                "dynamic_free",
            ] {
                metadata[flag] = serde_json::json!(true);
            }
            metadata["reviewed_by"] = serde_json::json!("insights");
            metadata["reviewed_at"] = serde_json::json!(Utc::now().to_rfc3339());
            if let Some(reason) = decision.reason.as_deref() {
                metadata["review_reason"] =
                    serde_json::json!(reason.chars().take(512).collect::<String>());
            }
            CacheReview::Approve {
                response_text,
                metadata,
            }
        } else if matches!(action.as_str(), "delete" | "reject")
            || (action == "approve" && cache_review_has_explicit_unsafe_flag(&decision))
        {
            CacheReview::Reject
        } else {
            continue;
        };
        match cache.apply_review(&entry, review).await? {
            CacheCas::Stale => {}
            CacheCas::Applied(None) => {
                stats.deleted += 1;
                stats.pending = stats.pending.saturating_sub(1);
            }
            CacheCas::Applied(Some(approved)) => {
                stats.approved += 1;
                stats.pending = stats.pending.saturating_sub(1);
                // This task owns the network request. Only the approved
                // revision can receive its vector; there is no detached job.
                if let Some(embedding) = &runtime.embedding {
                    let result = async {
                        if !cache
                            .authorize_work(&approved, CacheWork::Embedding)
                            .await?
                        {
                            return Ok(());
                        }
                        if let Some(batch) = embedding
                            .embed_profiled(&[approved.query_text.clone()])
                            .await?
                        {
                            anyhow::ensure!(
                                batch.vectors.len() == 1,
                                "cache embedding result count mismatch"
                            );
                            cache
                                .fill_embedding(&approved, &batch.profile, &batch.vectors[0])
                                .await?;
                        }
                        Ok::<(), anyhow::Error>(())
                    }
                    .await;
                    if let Err(error) = result {
                        tracing::debug!(cache_entry_id=%approved.version.cache_id,error=?error,
                            "approved cache embedding was not confirmed");
                    }
                }
            }
        }
    }
    Ok(stats)
}

async fn review_cache_entry_with_llm(
    llm: &LlmService,
    entry: &PendingCacheEntry,
) -> anyhow::Result<CacheReviewDecision> {
    let messages = vec![
        ChatMessage::new(
            "system",
            "You are an internal cache reviewer. Decide whether a pending assistant response is safe to enable for direct exact/semantic cache reuse. Approve only stable, timeless, tool-free, action-free, memory-independent answers that can be returned to the same user without fresh LLM reasoning. Delete anything that depends on current time, live data, user memory, tools, device state, actions, private context, uncertainty, or incomplete/empty content. You may refine wording, but must not add new facts. Return exactly one JSON object with keys: decision, refined_response, reason, safe_direct_return, memory_independent, tool_free, dynamic_free. decision must be approve, delete, or keep_pending.",
        ),
        ChatMessage::new(
            "user",
            format!(
                "Review this pending cache entry.\n\nUser query:\n{}\n\nCandidate assistant response:\n{}\n\nMetadata:\n{}",
                entry.query_text,
                entry.response_text,
                serde_json::to_string_pretty(&entry.metadata).unwrap_or_default()
            ),
        ),
    ];

    let raw = llm.text(&messages).await?;
    parse_cache_review_decision(&raw)
}

fn cache_review_approves_direct_return(decision: &CacheReviewDecision) -> bool {
    decision.safe_direct_return == Some(true)
        && decision.memory_independent == Some(true)
        && decision.tool_free == Some(true)
        && decision.dynamic_free == Some(true)
}

fn cache_review_has_explicit_unsafe_flag(decision: &CacheReviewDecision) -> bool {
    decision.safe_direct_return == Some(false)
        || decision.memory_independent == Some(false)
        || decision.tool_free == Some(false)
        || decision.dynamic_free == Some(false)
}

fn parse_cache_review_decision(raw: &str) -> anyhow::Result<CacheReviewDecision> {
    parse_llm_json_object(raw, "cache review")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_fenced_cache_review_decision() {
        let decision = parse_cache_review_decision(
            "```json\n{\"decision\":\"approve\",\"refined_response\":\"Stable answer.\",\"safe_direct_return\":true,\"memory_independent\":true,\"tool_free\":true,\"dynamic_free\":true}\n```",
        )
        .unwrap();

        assert_eq!(decision.decision, "approve");
        assert!(cache_review_approves_direct_return(&decision));
    }

    #[test]
    fn parses_cache_review_with_surrounding_text() {
        let decision = parse_cache_review_decision(
            "Here is the review:\n{\"decision\":\"delete\",\"reason\":\"live data\",\"safe_direct_return\":false,\"memory_independent\":true,\"tool_free\":true,\"dynamic_free\":false}",
        )
        .unwrap();

        assert_eq!(decision.decision, "delete");
        assert!(cache_review_has_explicit_unsafe_flag(&decision));
    }
}
