use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time;

use super::PendingTextPersistence;
use crate::config::{MemoryCaptureMode, MemorySection};
use crate::memory_policy::{
    detect_sensitive_memory, memory_content_equivalent, memory_merge_key, merge_memory_patch,
    normalize_memory_layer, normalize_memory_node_type,
};
use crate::module::ModuleRegistry;
use crate::providers::adapter::{
    ChatMessage, LlmAdapter, MemoryBackend, MemoryConversationIngest, MemoryConversationMessage,
    MemoryListOpts, MemoryNode,
};

const MEMORY_CAPTURE_SYSTEM_PROMPT: &str = r#"You decide whether a personal AI assistant should remember anything from one completed conversation turn.

Return exactly one JSON object with this schema:
{
  "memories": [
    {
      "content": "string",
      "memory_layer": "core|preference|episodic|relationship",
      "node_type": "fact|preference|relationship|goal|habit|event",
      "importance": 0.0,
      "confidence": 0.0,
      "reason": "string"
    }
  ]
}

Rules:
- Store only durable user facts, explicit preferences, stable profile details, long-term goals, recurring habits, or relationship context.
- Do not store one-off requests, transient task details, assistant guesses, secrets, current time/weather/news, tool outputs, or anything uncertain.
- If nothing should be remembered, return {"memories":[]}.
- Write memories in third person or neutral wording, without mentioning internal prompts or this policy.
- Be conservative. A personal assistant should remember less, but remember it well."#;

#[derive(Debug, Clone, Serialize)]
pub struct MemoryCaptureTrace {
    pub mode: MemoryCaptureMode,
    pub model: Option<String>,
    pub candidates: usize,
    pub stored: u32,
    pub skipped: u32,
    pub status: MemoryCaptureStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_traces: Vec<MemoryCandidateTrace>,
}

impl MemoryCaptureTrace {
    pub fn skipped(mode: MemoryCaptureMode, reason: impl Into<String>) -> Self {
        Self {
            mode,
            model: None,
            candidates: 0,
            stored: 0,
            skipped: 0,
            status: MemoryCaptureStatus::Skipped,
            reason: Some(reason.into()),
            candidate_traces: Vec::new(),
        }
    }

    fn failed(mode: MemoryCaptureMode, model: Option<String>, reason: impl Into<String>) -> Self {
        Self {
            mode,
            model,
            candidates: 0,
            stored: 0,
            skipped: 0,
            status: MemoryCaptureStatus::Failed,
            reason: Some(reason.into()),
            candidate_traces: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCaptureStatus {
    Completed,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryCandidateTrace {
    pub action: MemoryCandidateAction,
    pub content_preview: String,
    pub memory_layer: String,
    pub node_type: String,
    pub confidence: f32,
    pub importance: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCandidateAction {
    Stored,
    Updated,
    Skipped,
}

#[derive(Debug, Deserialize)]
struct MemoryCaptureResponse {
    #[serde(default)]
    memories: Vec<MemoryCaptureCandidate>,
}

#[derive(Debug, Deserialize)]
struct MemoryCaptureCandidate {
    content: String,
    #[serde(default)]
    memory_layer: Option<String>,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    node_type: Option<String>,
    #[serde(default)]
    importance: Option<f32>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    reason: Option<String>,
}

pub async fn capture_turn_memory(
    modules: &ModuleRegistry,
    pending: &PendingTextPersistence,
    memory_config: &MemorySection,
) -> MemoryCaptureTrace {
    let llm = modules
        .memory_capture_llm
        .as_deref()
        .unwrap_or_else(|| modules.minor_llm.as_ref());
    capture_turn_memory_with_backends(llm, modules.memory.as_ref(), pending, memory_config).await
}

pub async fn capture_turn_memory_with_backends(
    llm: &dyn LlmAdapter,
    memory: &dyn MemoryBackend,
    pending: &PendingTextPersistence,
    memory_config: &MemorySection,
) -> MemoryCaptureTrace {
    match memory_config.capture_mode {
        MemoryCaptureMode::None => {
            MemoryCaptureTrace::skipped(memory_config.capture_mode, "memory_capture_disabled")
        }
        MemoryCaptureMode::ExplicitToolOnly => {
            MemoryCaptureTrace::skipped(memory_config.capture_mode, "explicit_tool_only")
        }
        MemoryCaptureMode::ProviderInferAll => provider_infer_turn_memory(memory, pending).await,
        MemoryCaptureMode::LlmSelected => {
            llm_selected_turn_memory(llm, memory, pending, memory_config).await
        }
    }
}

async fn provider_infer_turn_memory(
    memory: &dyn MemoryBackend,
    pending: &PendingTextPersistence,
) -> MemoryCaptureTrace {
    let ingest = MemoryConversationIngest {
        run_id: Some(pending.session_id.to_string()),
        messages: vec![
            MemoryConversationMessage {
                role: "user".to_string(),
                content: pending.query_text.clone(),
                name: Some(pending.user_id_text.clone()),
            },
            MemoryConversationMessage {
                role: "assistant".to_string(),
                content: pending.response_text.clone(),
                name: None,
            },
        ],
        metadata: serde_json::json!({
            "source": "pipeline",
            "capture_mode": "provider_infer_all",
            "channel_id": pending.channel_id,
            "conversation_id": pending.session_id,
        }),
    };

    match memory.ingest_conversation(pending.user_id, ingest).await {
        Ok(_) => MemoryCaptureTrace {
            mode: MemoryCaptureMode::ProviderInferAll,
            model: None,
            candidates: 0,
            stored: 0,
            skipped: 0,
            status: MemoryCaptureStatus::Completed,
            reason: None,
            candidate_traces: Vec::new(),
        },
        Err(error) => {
            tracing::warn!(
                user_id = %pending.user_id,
                session_id = %pending.session_id,
                error = ?error,
                "failed to ingest conversation into memory backend"
            );
            MemoryCaptureTrace::failed(
                MemoryCaptureMode::ProviderInferAll,
                None,
                "provider_ingest_failed",
            )
        }
    }
}

async fn llm_selected_turn_memory(
    llm: &dyn LlmAdapter,
    memory: &dyn MemoryBackend,
    pending: &PendingTextPersistence,
    memory_config: &MemorySection,
) -> MemoryCaptureTrace {
    let model = llm.name().to_string();
    if llm.name() == "noop_llm" {
        tracing::debug!("memory capture skipped because minor LLM is unavailable");
        return MemoryCaptureTrace::skipped(MemoryCaptureMode::LlmSelected, "llm_unavailable");
    }

    let messages = vec![
        ChatMessage::new("system", MEMORY_CAPTURE_SYSTEM_PROMPT),
        ChatMessage::new(
            "user",
            format!(
                "User id: {}\nChannel id: {}\nConversation id: {}\n\nUser message:\n{}\n\nAssistant response:\n{}",
                pending.user_id_text,
                pending.channel_id,
                pending.session_id,
                pending.query_text,
                pending.response_text
            ),
        ),
    ];
    let response = match time::timeout(
        Duration::from_secs(memory_config.capture_timeout_sec.max(1)),
        llm.chat(&messages),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            tracing::debug!(error = ?error, "memory capture LLM failed");
            return MemoryCaptureTrace::failed(
                MemoryCaptureMode::LlmSelected,
                Some(model),
                "llm_failed",
            );
        }
        Err(_) => {
            tracing::debug!("memory capture LLM timed out");
            return MemoryCaptureTrace::failed(
                MemoryCaptureMode::LlmSelected,
                Some(model),
                "llm_timeout",
            );
        }
    };

    let Some(parsed) = parse_memory_capture_response(&response) else {
        tracing::debug!("memory capture LLM returned invalid JSON");
        return MemoryCaptureTrace::failed(
            MemoryCaptureMode::LlmSelected,
            Some(model),
            "invalid_json",
        );
    };
    let candidate_count = parsed.memories.len();
    let (stored, skipped, candidate_traces) =
        store_memory_capture_candidates(memory, pending, memory_config, parsed.memories).await;
    tracing::debug!(
        user_id = %pending.user_id,
        session_id = %pending.session_id,
        model,
        candidate_count,
        stored,
        skipped,
        "memory capture completed"
    );
    MemoryCaptureTrace {
        mode: memory_config.capture_mode,
        model: Some(model),
        candidates: candidate_count,
        stored,
        skipped,
        status: MemoryCaptureStatus::Completed,
        reason: None,
        candidate_traces,
    }
}

async fn store_memory_capture_candidates(
    memory: &dyn MemoryBackend,
    pending: &PendingTextPersistence,
    memory_config: &MemorySection,
    candidates: Vec<MemoryCaptureCandidate>,
) -> (u32, u32, Vec<MemoryCandidateTrace>) {
    let mut stored = 0;
    let mut skipped = 0;
    let mut traces = Vec::new();
    let min_confidence = memory_config.capture_min_confidence.clamp(0.0, 1.0);
    for candidate in candidates {
        let content = candidate.content.trim();
        let confidence = candidate.confidence.unwrap_or(1.0).clamp(0.0, 1.0);
        let layer = candidate
            .memory_layer
            .or(candidate.layer)
            .map(|layer| normalize_memory_layer(&layer))
            .unwrap_or_else(|| "core".to_string());
        let node_type = candidate
            .node_type
            .as_deref()
            .map(normalize_memory_node_type)
            .unwrap_or_else(|| "fact".to_string());
        let importance = candidate.importance.unwrap_or(0.6).clamp(0.0, 1.0);
        let reason = candidate.reason;

        if content.is_empty() {
            skipped += 1;
            traces.push(candidate_trace(
                MemoryCandidateAction::Skipped,
                content,
                layer,
                node_type,
                confidence,
                importance,
                reason,
                Some("empty_content".to_string()),
            ));
            continue;
        }
        if memory_config.sensitive_filter_enabled
            && let Some(sensitive) = detect_sensitive_memory(content)
        {
            skipped += 1;
            traces.push(candidate_trace(
                MemoryCandidateAction::Skipped,
                content,
                layer,
                node_type,
                confidence,
                importance,
                reason,
                Some(format!("sensitive_memory_blocked:{}", sensitive.category)),
            ));
            continue;
        }
        if confidence < min_confidence {
            skipped += 1;
            traces.push(candidate_trace(
                MemoryCandidateAction::Skipped,
                content,
                layer,
                node_type,
                confidence,
                importance,
                reason,
                Some("low_confidence".to_string()),
            ));
            continue;
        }

        let metadata = serde_json::json!({
            "source": "pipeline",
            "capture_mode": "llm_selected",
            "confidence": confidence,
            "reason": reason.clone(),
            "channel_id": pending.channel_id,
            "conversation_id": pending.session_id,
            "user_id_text": pending.user_id_text,
        });
        let incoming = MemoryNode {
            id: None,
            memory_layer: layer.clone(),
            node_type: node_type.clone(),
            content: content.to_string(),
            metadata,
            importance,
        };
        match upsert_memory_candidate(memory, pending, &incoming).await {
            Ok(MemoryUpsertAction::Stored) => {
                stored += 1;
                traces.push(candidate_trace(
                    MemoryCandidateAction::Stored,
                    content,
                    layer,
                    node_type,
                    confidence,
                    importance,
                    reason,
                    None,
                ));
            }
            Ok(MemoryUpsertAction::Updated) => {
                stored += 1;
                traces.push(candidate_trace(
                    MemoryCandidateAction::Updated,
                    content,
                    layer,
                    node_type,
                    confidence,
                    importance,
                    reason,
                    None,
                ));
            }
            Ok(MemoryUpsertAction::SkippedDuplicate) => {
                skipped += 1;
                traces.push(candidate_trace(
                    MemoryCandidateAction::Skipped,
                    content,
                    layer,
                    node_type,
                    confidence,
                    importance,
                    reason,
                    Some("duplicate_existing_memory".to_string()),
                ));
            }
            Err(error) => {
                skipped += 1;
                tracing::warn!(
                    user_id = %pending.user_id,
                    session_id = %pending.session_id,
                    error = ?error,
                    "failed to store selected memory candidate"
                );
                traces.push(candidate_trace(
                    MemoryCandidateAction::Skipped,
                    content,
                    layer,
                    node_type,
                    confidence,
                    importance,
                    reason,
                    Some("store_failed".to_string()),
                ));
            }
        }
    }
    (stored, skipped, traces)
}

enum MemoryUpsertAction {
    Stored,
    Updated,
    SkippedDuplicate,
}

async fn upsert_memory_candidate(
    memory: &dyn MemoryBackend,
    pending: &PendingTextPersistence,
    incoming: &MemoryNode,
) -> anyhow::Result<MemoryUpsertAction> {
    let merge_key = memory_merge_key(
        &incoming.memory_layer,
        &incoming.node_type,
        &incoming.content,
    );
    let existing = memory
        .list(
            pending.user_id,
            MemoryListOpts {
                max_results: Some(200),
                layers: Some(vec![incoming.memory_layer.clone()]),
            },
        )
        .await?;

    for node in existing {
        if memory_content_equivalent(&node.content, &incoming.content) {
            return Ok(MemoryUpsertAction::SkippedDuplicate);
        }
        let Some(existing_id) = node.id.as_deref() else {
            continue;
        };
        if merge_key.is_some()
            && merge_key == memory_merge_key(&node.memory_layer, &node.node_type, &node.content)
        {
            memory
                .update(existing_id, merge_memory_patch(&node, incoming))
                .await?;
            return Ok(MemoryUpsertAction::Updated);
        }
    }

    memory.store(pending.user_id, incoming.clone()).await?;
    Ok(MemoryUpsertAction::Stored)
}

fn candidate_trace(
    action: MemoryCandidateAction,
    content: &str,
    memory_layer: String,
    node_type: String,
    confidence: f32,
    importance: f32,
    reason: Option<String>,
    skip_reason: Option<String>,
) -> MemoryCandidateTrace {
    MemoryCandidateTrace {
        action,
        content_preview: preview_for_trace(content, 160),
        memory_layer,
        node_type,
        confidence,
        importance,
        reason,
        skip_reason,
    }
}

fn parse_memory_capture_response(response: &str) -> Option<MemoryCaptureResponse> {
    let json = json_object_slice(response)?;
    serde_json::from_str(json).ok()
}

fn json_object_slice(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    for (offset, ch) in text[start..].char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&text[start..start + offset + ch.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

fn preview_for_trace(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let mut preview = trimmed.chars().take(max_chars).collect::<String>();
    if trimmed.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_with_surrounding_text() {
        let parsed = parse_memory_capture_response(
            "Here is the result:\n{\"memories\":[{\"content\":\"The user likes concise replies.\",\"confidence\":0.9}]}",
        )
        .expect("capture response should parse");

        assert_eq!(parsed.memories.len(), 1);
        assert_eq!(
            parsed.memories[0].content,
            "The user likes concise replies."
        );
    }
}
