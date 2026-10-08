//! Authorized memory extraction, consolidation, reflection, and habit derivation.

use crate::config::InsightsSection;
use crate::entity::chat_session;
use crate::modules::memory::context::{ContextError, ContextService, ContextUse};
use crate::modules::memory::policy::detect_sensitive_memory;
use crate::modules::memory::{
    MemoryBatch, MemoryListOpts, MemoryNode, MemoryPatch, MemoryService, MemoryWriteContext,
    MemoryWriteOrigin,
};
use crate::modules::platform::lifecycle::ModuleLifecycle;
use crate::providers::adapter::{ChatMessage, LlmAdapter};
use crate::providers::llm::response::{join_model_labels, parse_llm_json_object};
use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Statement, TransactionTrait,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub(crate) struct MemoryMaintenanceUser {
    pub(crate) user_id: Uuid,
    pub(crate) user_id_text: String,
    pub(crate) channel_id: Option<String>,
    pub(crate) session_id: Option<Uuid>,
    pub(crate) last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MemoryMaintenanceStats {
    pub(crate) created: u32,
    pub(crate) updated: u32,
    pub(crate) retired: u32,
    pub(crate) ingested: u32,
    pub(crate) pending: u32,
    pub(crate) summary: String,
    pub(crate) model_used: String,
}

#[derive(Debug, Deserialize)]
struct MemoryExtractionResponse {
    #[serde(default)]
    memories: Vec<MemoryCandidate>,
}

#[derive(Debug, Deserialize)]
struct MemoryCandidate {
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryConsolidationResponse {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    updates: Vec<MemoryUpdateDecision>,
    #[serde(default)]
    retirements: Vec<MemoryRetirementDecision>,
    #[serde(default)]
    creates: Vec<MemoryCandidate>,
}

#[derive(Debug, Deserialize)]
struct MemoryUpdateDecision {
    id: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    importance: Option<f32>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryRetirementDecision {
    id: String,
    status: RetirementStatus,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RetirementStatus {
    Superseded,
    Rejected,
}

async fn validate_insight_read_policy(
    db: &DatabaseConnection,
    context: &MemoryWriteContext,
) -> anyhow::Result<bool> {
    let usage = match context.origin {
        MemoryWriteOrigin::Automatic => ContextUse::AutomaticMemoryWrite,
        MemoryWriteOrigin::Maintenance => ContextUse::MemoryMaintenance,
        _ => anyhow::bail!("invalid background insight origin"),
    };
    let expected = context
        .policy_version
        .ok_or_else(|| anyhow::anyhow!("insights requires persistent source policy"))?;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let tx = db.begin().await?;
        let allowed = match ContextService::policy_in_transaction(&tx, Some(expected), usage).await
        {
            Ok(_) => true,
            Err(error)
                if matches!(
                    error.downcast_ref::<ContextError>(),
                    Some(ContextError::Revoked(_))
                ) =>
            {
                false
            }
            Err(error) => return Err(error),
        };
        tx.commit().await?;
        Ok::<bool, anyhow::Error>(allowed)
    })
    .await
    .map_err(|_| anyhow::anyhow!("insights policy validation timed out"))?
}

pub(crate) async fn extract_memories_from_recent_conversation(
    db: &DatabaseConnection,
    memory: Option<Arc<MemoryService>>,
    llm: Option<Arc<dyn LlmAdapter>>,
    user: &MemoryMaintenanceUser,
    message_limit: u64,
    config: &InsightsSection,
) -> anyhow::Result<MemoryMaintenanceStats> {
    let Some(memory) = memory else {
        return Ok(MemoryMaintenanceStats {
            summary: "Extraction skipped: memory system is unavailable.".to_string(),
            model_used: "none".to_string(),
            ..MemoryMaintenanceStats::default()
        });
    };

    let mut context = memory.write_snapshot(user.user_id).await?;
    context.origin = MemoryWriteOrigin::Automatic;
    if !validate_insight_read_policy(db, &context).await? {
        return Ok(MemoryMaintenanceStats {
            summary: "Memory insights skipped: current policy disallows this phase.".into(),
            model_used: memory.name().into(),
            ..Default::default()
        });
    }
    let messages = load_recent_messages(db, user, message_limit, &mut context).await?;
    if messages.is_empty() {
        return Ok(MemoryMaintenanceStats {
            summary: "Extraction skipped: no recent conversation messages.".to_string(),
            model_used: memory.name().to_string(),
            ..MemoryMaintenanceStats::default()
        });
    }

    let Some(llm) = llm else {
        return Ok(MemoryMaintenanceStats {
            summary: "Extraction skipped: LLM is unavailable.".to_string(),
            model_used: memory.name().to_string(),
            ..MemoryMaintenanceStats::default()
        });
    };
    let mut stats = MemoryMaintenanceStats {
        model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
        ..MemoryMaintenanceStats::default()
    };

    let captured =
        ContextService::materialize_memory_capture(db, &context, MemoryWriteOrigin::Automatic)
            .await?;
    context = captured
        .lease()
        .memory_write_context(context, MemoryWriteOrigin::Automatic)?;
    context = memory
        .bind_context(context, captured.lease().id(), MemoryWriteOrigin::Automatic)
        .await?;
    let messages = captured
        .messages()
        .iter()
        .map(|row| row.message().clone())
        .collect::<Vec<_>>();
    let transcript = transcript_for_prompt(&messages);
    memory.validate_write_context(&context).await?;
    let response = llm
        .chat(&[
            ChatMessage::new("system", config.memory_extraction_prompt.clone()),
            ChatMessage::new(
                "user",
                format!(
                    "User id: {}\nRecent conversation transcript:\n{}",
                    user.user_id_text, transcript
                ),
            ),
        ])
        .await?;
    let extracted = parse_memory_extraction_response(&response)?;
    stats.created += store_memory_candidates(
        memory.as_ref(),
        user,
        &context,
        extracted.memories,
        "extraction",
    )
    .await?;
    stats.summary = format!(
        "Extraction stored {} selected memory candidates.",
        stats.created
    );
    Ok(stats)
}

pub(crate) async fn consolidate_memories(
    db: &DatabaseConnection,
    memory: Option<Arc<MemoryService>>,
    llm: Option<Arc<dyn LlmAdapter>>,
    user: &MemoryMaintenanceUser,
    limit: u32,
    config: &InsightsSection,
) -> anyhow::Result<MemoryMaintenanceStats> {
    let Some(memory) = memory else {
        return Ok(MemoryMaintenanceStats {
            summary: "Memory consolidation skipped: memory system is unavailable.".to_string(),
            model_used: "none".to_string(),
            ..MemoryMaintenanceStats::default()
        });
    };
    let Some(llm) = llm else {
        return Ok(MemoryMaintenanceStats {
            summary: "Memory consolidation skipped: LLM is unavailable.".to_string(),
            model_used: memory.name().to_string(),
            ..MemoryMaintenanceStats::default()
        });
    };

    let mut context = memory.write_snapshot(user.user_id).await?;
    context.origin = MemoryWriteOrigin::Maintenance;
    if !validate_insight_read_policy(db, &context).await? {
        return Ok(MemoryMaintenanceStats {
            summary: "Memory insights skipped: current policy disallows this phase.".into(),
            model_used: memory.name().into(),
            ..Default::default()
        });
    }
    let memories = memory
        .list(
            user.user_id,
            MemoryListOpts {
                max_results: Some(limit),
                layers: None,
            },
        )
        .await?;
    context.source_memories = source_versions(&memories);
    if memories.is_empty() {
        return Ok(MemoryMaintenanceStats {
            summary: "Memory consolidation skipped: no memories found.".to_string(),
            model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
            ..MemoryMaintenanceStats::default()
        });
    }

    let captured =
        ContextService::materialize_memory_capture(db, &context, MemoryWriteOrigin::Maintenance)
            .await?;
    context = captured
        .lease()
        .memory_write_context(context, MemoryWriteOrigin::Maintenance)?;
    context = memory
        .bind_context(
            context,
            captured.lease().id(),
            MemoryWriteOrigin::Maintenance,
        )
        .await?;
    let memories = captured.memories().to_vec();
    memory.validate_write_context(&context).await?;
    let response = llm
        .chat(&[
            ChatMessage::new("system", config.memory_consolidation_prompt.clone()),
            ChatMessage::new(
                "user",
                format!(
                    "User id: {}\nCurrent memories:\n{}",
                    user.user_id_text,
                    memories_for_prompt(&memories)
                ),
            ),
        ])
        .await?;
    apply_consolidation_response(
        memory.as_ref(),
        &context,
        &memories,
        user,
        &response,
        llm.name(),
    )
    .await
}

async fn apply_consolidation_response(
    memory: &MemoryService,
    context: &MemoryWriteContext,
    memories: &[MemoryNode],
    user: &MemoryMaintenanceUser,
    response: &str,
    llm_name: &str,
) -> anyhow::Result<MemoryMaintenanceStats> {
    let decision = parse_memory_consolidation_response(response)?;
    let _proposed_summary = decision.summary;
    let mut batch = MemoryBatch::default();
    let mut selected = std::collections::HashSet::new();
    let updated = decision.updates.len() as u32;
    let retired = decision.retirements.len() as u32;
    for update in decision.updates {
        anyhow::ensure!(
            selected.insert(update.id.clone()),
            "duplicate consolidation target"
        );
        let source = memories
            .iter()
            .find(|m| m.id.as_deref() == Some(update.id.as_str()))
            .ok_or_else(|| anyhow::anyhow!("consolidation references an unknown memory"))?;
        if let Some(content) = &update.content {
            anyhow::ensure!(!content.trim().is_empty(), "empty consolidation content");
            anyhow::ensure!(
                detect_sensitive_memory(content).is_none(),
                "sensitive consolidation content rejected"
            );
        }
        anyhow::ensure!(
            update.content.is_some() || update.importance.is_some(),
            "empty consolidation update"
        );
        batch.updates.push((
            update.id,
            MemoryPatch {
                expected_revision: Some(source.revision),
                content: update.content.map(|content| content.trim().to_owned()),
                importance: update.importance.map(clamp_score),
                metadata: Some(serde_json::json!({
                    "source": "insights", "phase": "consolidation", "reason": update.reason,
                    "updated_at": Utc::now().to_rfc3339(),
                })),
                ..MemoryPatch::default()
            },
        ));
    }
    for retirement in decision.retirements {
        anyhow::ensure!(
            selected.insert(retirement.id.clone()),
            "duplicate consolidation target"
        );
        anyhow::ensure!(
            !retirement.reason.trim().is_empty(),
            "retirement requires a reason"
        );
        let source = memories
            .iter()
            .find(|m| m.id.as_deref() == Some(retirement.id.as_str()))
            .ok_or_else(|| anyhow::anyhow!("retirement references an unknown memory"))?;
        let status = match retirement.status {
            RetirementStatus::Superseded => "superseded",
            RetirementStatus::Rejected => "rejected",
        };
        batch.updates.push((
            retirement.id,
            MemoryPatch {
                expected_revision: Some(source.revision),
                status: Some(status.into()),
                metadata: Some(serde_json::json!({
                    "source": "insights", "phase": "consolidation", "reason": retirement.reason,
                    "retired_at": Utc::now().to_rfc3339(),
                })),
                ..MemoryPatch::default()
            },
        ));
    }
    batch.creates = prepare_memory_candidates(user, decision.creates, "consolidation")?;
    let committed = memory
        .apply_batch_guarded(context, batch)
        .await
        .map_err(|error| anyhow::anyhow!("consolidation batch was not confirmed: {error}"))?;
    let created = committed.created_ids.len() as u32;
    anyhow::ensure!(
        committed.updated == (updated + retired) as usize,
        "memory batch receipt count mismatch"
    );
    Ok(MemoryMaintenanceStats {
        created,
        updated,
        retired,
        pending: (memories.len() as u32).saturating_sub(updated + retired),
        model_used: join_model_labels(&[memory.name().to_string(), llm_name.to_string()]),
        summary: format!(
            "Consolidation committed {updated} updates, {retired} retirements and {created} creates atomically."
        ),
        ..MemoryMaintenanceStats::default()
    })
}

pub(crate) async fn reflect_on_user_context(
    db: &DatabaseConnection,
    memory: Option<Arc<MemoryService>>,
    llm: Option<Arc<dyn LlmAdapter>>,
    user: &MemoryMaintenanceUser,
    memory_limit: u32,
    config: &InsightsSection,
) -> anyhow::Result<MemoryMaintenanceStats> {
    let Some(memory) = memory else {
        return Ok(MemoryMaintenanceStats {
            summary: "Reflection skipped: memory system is unavailable.".to_string(),
            model_used: "none".to_string(),
            ..MemoryMaintenanceStats::default()
        });
    };
    let Some(llm) = llm else {
        return Ok(MemoryMaintenanceStats {
            summary: "Reflection skipped: LLM is unavailable.".to_string(),
            model_used: memory.name().to_string(),
            ..MemoryMaintenanceStats::default()
        });
    };

    let mut context = memory.write_snapshot(user.user_id).await?;
    context.origin = MemoryWriteOrigin::Automatic;
    if !validate_insight_read_policy(db, &context).await? {
        return Ok(MemoryMaintenanceStats {
            summary: "Memory insights skipped: current policy disallows this phase.".into(),
            model_used: memory.name().into(),
            ..Default::default()
        });
    }
    let memories = memory
        .list(
            user.user_id,
            MemoryListOpts {
                max_results: Some(memory_limit),
                layers: None,
            },
        )
        .await?;
    context.source_memories = source_versions(&memories);
    let messages = load_recent_messages(db, user, 16, &mut context).await?;

    if memories.is_empty() && messages.is_empty() {
        return Ok(MemoryMaintenanceStats {
            summary: "Reflection skipped: no memory or conversation context found.".to_string(),
            model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
            ..MemoryMaintenanceStats::default()
        });
    }

    let captured =
        ContextService::materialize_memory_capture(db, &context, MemoryWriteOrigin::Automatic)
            .await?;
    context = captured
        .lease()
        .memory_write_context(context, MemoryWriteOrigin::Automatic)?;
    context = memory
        .bind_context(context, captured.lease().id(), MemoryWriteOrigin::Automatic)
        .await?;
    let messages = captured
        .messages()
        .iter()
        .map(|row| row.message().clone())
        .collect::<Vec<_>>();
    let memories = captured.memories().to_vec();
    memory.validate_write_context(&context).await?;
    let response = llm
        .chat(&[
            ChatMessage::new("system", config.reflection_prompt.clone()),
            ChatMessage::new(
                "user",
                format!(
                    "User id: {}\nCurrent memories:\n{}\n\nRecent conversation:\n{}",
                    user.user_id_text,
                    memories_for_prompt(&memories),
                    transcript_for_prompt(&messages)
                ),
            ),
        ])
        .await?;
    let extracted = parse_memory_extraction_response(&response)?;
    let mut stats = MemoryMaintenanceStats {
        model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
        ..MemoryMaintenanceStats::default()
    };
    stats.created += store_memory_candidates(
        memory.as_ref(),
        user,
        &context,
        extracted.memories,
        "reflection",
    )
    .await?;
    stats.summary = format!("Reflection stored {} durable insights.", stats.created);
    Ok(stats)
}

pub(crate) async fn detect_habits_from_context(
    db: &DatabaseConnection,
    memory: Option<Arc<MemoryService>>,
    llm: Option<Arc<dyn LlmAdapter>>,
    user: &MemoryMaintenanceUser,
    memory_limit: u32,
    config: &InsightsSection,
) -> anyhow::Result<MemoryMaintenanceStats> {
    let Some(memory) = memory else {
        return Ok(MemoryMaintenanceStats {
            summary: "Habit detection skipped: memory system is unavailable.".to_string(),
            model_used: "none".to_string(),
            ..MemoryMaintenanceStats::default()
        });
    };
    let Some(llm) = llm else {
        return Ok(MemoryMaintenanceStats {
            summary: "Habit detection skipped: LLM is unavailable.".to_string(),
            model_used: memory.name().to_string(),
            ..MemoryMaintenanceStats::default()
        });
    };

    let mut context = memory.write_snapshot(user.user_id).await?;
    context.origin = MemoryWriteOrigin::Automatic;
    if !validate_insight_read_policy(db, &context).await? {
        return Ok(MemoryMaintenanceStats {
            summary: "Memory insights skipped: current policy disallows this phase.".into(),
            model_used: memory.name().into(),
            ..Default::default()
        });
    }
    let memories = memory
        .list(
            user.user_id,
            MemoryListOpts {
                max_results: Some(memory_limit),
                layers: None,
            },
        )
        .await?;
    context.source_memories = source_versions(&memories);
    let messages = load_recent_messages(db, user, 32, &mut context).await?;

    if memories.is_empty() && messages.is_empty() {
        return Ok(MemoryMaintenanceStats {
            summary: "Habit detection skipped: no memory or conversation context found."
                .to_string(),
            model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
            ..MemoryMaintenanceStats::default()
        });
    }

    let captured =
        ContextService::materialize_memory_capture(db, &context, MemoryWriteOrigin::Automatic)
            .await?;
    context = captured
        .lease()
        .memory_write_context(context, MemoryWriteOrigin::Automatic)?;
    context = memory
        .bind_context(context, captured.lease().id(), MemoryWriteOrigin::Automatic)
        .await?;
    let messages = captured
        .messages()
        .iter()
        .map(|row| row.message().clone())
        .collect::<Vec<_>>();
    let memories = captured.memories().to_vec();
    memory.validate_write_context(&context).await?;
    let response = llm
        .chat(&[
            ChatMessage::new("system", config.habit_detection_prompt.clone()),
            ChatMessage::new(
                "user",
                format!(
                    "User id: {}\nCurrent memories:\n{}\n\nRecent conversation:\n{}",
                    user.user_id_text,
                    memories_for_prompt(&memories),
                    transcript_for_prompt(&messages)
                ),
            ),
        ])
        .await?;
    let extracted = parse_memory_extraction_response(&response)?;
    let mut stats = MemoryMaintenanceStats {
        model_used: join_model_labels(&[memory.name().to_string(), llm.name().to_string()]),
        ..MemoryMaintenanceStats::default()
    };
    stats.created += store_memory_candidates(
        memory.as_ref(),
        user,
        &context,
        extracted.memories,
        "habit_detection",
    )
    .await?;
    stats.summary = format!("Habit detection stored {} durable habits.", stats.created);
    Ok(stats)
}

async fn store_memory_candidates(
    memory: &MemoryService,
    user: &MemoryMaintenanceUser,
    context: &MemoryWriteContext,
    candidates: Vec<MemoryCandidate>,
    phase: &str,
) -> anyhow::Result<u32> {
    let creates = prepare_memory_candidates(user, candidates, phase)?;
    let result = memory
        .apply_batch_guarded(
            context,
            MemoryBatch {
                creates,
                ..Default::default()
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("{phase} batch was not confirmed: {error}"))?;
    Ok(result.created_ids.len() as u32)
}

fn prepare_memory_candidates(
    user: &MemoryMaintenanceUser,
    candidates: Vec<MemoryCandidate>,
    phase: &str,
) -> anyhow::Result<Vec<MemoryNode>> {
    anyhow::ensure!(candidates.len() <= 64, "too many insight memory candidates");
    let mut nodes = Vec::new();
    for candidate in candidates {
        let content = candidate.content.trim();
        let confidence = clamp_score(candidate.confidence.unwrap_or(1.0));
        if content.is_empty() || confidence < 0.6 || detect_sensitive_memory(content).is_some() {
            continue;
        }
        let layer = candidate
            .memory_layer
            .or(candidate.layer)
            .map(|layer| normalize_memory_layer(&layer))
            .unwrap_or_else(|| "core".into());
        let node_type = candidate
            .node_type
            .as_deref()
            .map(normalize_memory_node_type)
            .unwrap_or_else(|| "fact".into());
        let importance = clamp_score(candidate.importance.unwrap_or(0.6));
        let metadata = serde_json::json!({
            "source": "insights", "phase": phase, "confidence": confidence,
            "reason": candidate.reason, "user_id_text": user.user_id_text,
            "channel_id": user.channel_id, "created_at": Utc::now().to_rfc3339(),
        });
        nodes.push(
            MemoryNode::new(layer, node_type, content, importance)
                .with_metadata(metadata)
                .with_runtime_fields("semantic", "user", confidence, importance, "internal"),
        );
    }
    Ok(nodes)
}

async fn load_recent_messages(
    db: &DatabaseConnection,
    user: &MemoryMaintenanceUser,
    limit: u64,
    context: &mut MemoryWriteContext,
) -> anyhow::Result<Vec<ChatMessage>> {
    let session_id = match user.session_id {
        Some(id) => Some(id),
        None => find_latest_session_for_user(db, user).await?,
    };
    let Some(session_id) = session_id else {
        return Ok(Vec::new());
    };
    let mut rows = db.query_all(Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,
        "SELECT m.id,m.content,m.user_id FROM chat_messages m JOIN chat_sessions s ON s.id=m.session_id WHERE s.id=$1 AND s.creator_id=$2 AND m.user_id=$2 AND m.role='user' AND m.source_received_at IS NOT NULL AND m.source_privacy_epoch=$3 ORDER BY m.created_at DESC,m.id DESC LIMIT $4",
        vec![session_id.into(),user.user_id.to_string().into(),context.privacy_epoch.into(),(limit.clamp(1,100) as i64).into()])).await?;
    rows.reverse();
    let mut messages = Vec::with_capacity(rows.len());
    for row in rows {
        context.source_event_ids.push(row.try_get("", "id")?);
        messages.push(ChatMessage {
            role: "user".into(),
            content: row.try_get("", "content")?,
            name: row.try_get("", "user_id")?,
            ..Default::default()
        });
    }
    Ok(messages)
}

async fn find_latest_session_for_user(
    db: &DatabaseConnection,
    user: &MemoryMaintenanceUser,
) -> anyhow::Result<Option<Uuid>> {
    let mut condition = Condition::any()
        .add(chat_session::Column::CreatorId.eq(Some(user.user_id_text.clone())))
        .add(chat_session::Column::CreatorId.eq(Some(user.user_id.to_string())));
    if let Some(channel_id) = &user.channel_id {
        condition = condition.add(
            Condition::all()
                .add(chat_session::Column::ChannelId.eq(channel_id.clone()))
                .add(chat_session::Column::CreatorId.eq(Some(user.user_id_text.clone()))),
        );
    }
    Ok(chat_session::Entity::find()
        .filter(condition)
        .order_by_desc(chat_session::Column::UpdatedAt)
        .limit(1)
        .one(db)
        .await?
        .map(|session| session.id))
}

fn parse_memory_extraction_response(raw: &str) -> anyhow::Result<MemoryExtractionResponse> {
    parse_llm_json_object(raw, "memory extraction")
}

fn parse_memory_consolidation_response(raw: &str) -> anyhow::Result<MemoryConsolidationResponse> {
    parse_llm_json_object(raw, "memory consolidation")
}
// CapturedMemoryContext bounds the complete canonical inputs before issuing
// its lease. Formatting must not silently drop a declared source afterwards.
fn transcript_for_prompt(messages: &[ChatMessage]) -> String {
    let mut output = String::new();
    for message in messages {
        let role = message.role.trim();
        let name = message.name.as_deref().unwrap_or_default().trim();
        let content = message.content.trim();
        if content.is_empty() {
            continue;
        }
        if !name.is_empty() {
            output.push_str(&format!("{role}({name}): {content}\n"));
        } else {
            output.push_str(&format!("{role}: {content}\n"));
        }
    }
    output
}

fn memories_for_prompt(memories: &[MemoryNode]) -> String {
    memories
        .iter()
        .enumerate()
        .map(|(index, memory)| {
            format!(
                "{}. id={} layer={} type={} importance={:.2}: {}",
                index + 1,
                memory.id.as_deref().unwrap_or(""),
                memory.memory_layer,
                memory.node_type,
                memory.importance,
                memory.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn clamp_score(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.5
    }
}

fn normalize_memory_layer(layer: &str) -> String {
    match layer.trim().to_ascii_lowercase().as_str() {
        "preference" | "preferences" => "preference".to_string(),
        "episode" | "episodic" | "event" => "episodic".to_string(),
        "reflection" | "insight" | "insights" => "reflection".to_string(),
        "core" | "profile" | "fact" => "core".to_string(),
        other if !other.is_empty() => other.to_string(),
        _ => "core".to_string(),
    }
}

fn normalize_memory_node_type(node_type: &str) -> String {
    match node_type.trim().to_ascii_lowercase().as_str() {
        "preference" | "fact" | "relationship" | "goal" | "habit" | "event" | "insight" => {
            node_type.trim().to_ascii_lowercase()
        }
        other if !other.is_empty() => other.to_string(),
        _ => "fact".to_string(),
    }
}
fn source_versions(memories: &[MemoryNode]) -> Vec<(String, i64)> {
    memories
        .iter()
        .filter_map(|memory| memory.id.clone().map(|id| (id, memory.revision)))
        .collect()
}

#[cfg(test)]
mod tests;
