use super::cache::CacheWrite;
use super::debug::debug_component_json;
use super::delivery;
use super::services::InteractionServices;
use super::trace::AgentTrace;
use super::turn::VoiceTurn;
use crate::config::MemorySection;
use crate::event::{EventBus, ServerEvent};
use crate::modules::memory::capture::{CaptureTurn, MemoryCaptureTrace, capture_turn_memory};
use crate::modules::memory::context::ContextBinding;
use crate::{
    modules::interaction::{cache::CacheEntry, conversation::ConversationEventEntry},
    providers::adapter::ChatMessage,
};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct PendingTextPersistence {
    pub context: ContextBinding,
    pub source_received_at: chrono::DateTime<chrono::Utc>,
    pub source_privacy_epoch: i64,
    pub source_event_id: Option<Uuid>,
    pub response_id: Uuid,
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub user_id_text: String,
    pub query_text: String,
    pub response_text: String,
    pub channel_id: String,
    pub cache_write: Option<CacheWrite>,
}

#[derive(Debug, Clone)]
pub(super) struct InterruptedUserContext {
    pub(super) session_id: Uuid,
    pub(super) source_event_id: Option<Uuid>,
    pub(super) turn_id: Uuid,
    pub(super) user_id: Uuid,
    pub(super) user_id_text: String,
    pub(super) query_text: String,
    pub(super) channel_id: String,
    pub(super) reason: String,
    pub(super) stage: &'static str,
}

pub(super) async fn persist_text_interaction(
    modules: &InteractionServices,
    pending: PendingTextPersistence,
    memory_config: &MemorySection,
    event_bus: &EventBus,
    trace: Option<&mut AgentTrace>,
) {
    let completed_channel_id = pending.channel_id.clone();
    let completed_user_id = pending.user_id_text.clone();
    let completed_user_uuid = pending.user_id;
    let completed_session_id = pending.session_id;

    if pending.source_event_id.is_none()
        || pending.context.source_event_id != pending.source_event_id
        || pending.context.owner_id != pending.user_id
        || pending.context.source_privacy_epoch != pending.source_privacy_epoch
    {
        tracing::error!(response_id=%pending.response_id, "derived response lost its original context");
        return;
    }

    if let Err(error) = modules
        .conversation
        .commit_response(
            pending.user_id,
            pending.session_id,
            pending.response_id,
            ChatMessage {
                role: "assistant".into(),
                content: pending.response_text.clone(),
                name: None,
                ..ChatMessage::default()
            },
        )
        .await
    {
        tracing::error!(error = %error, session_id = %pending.session_id,
            "assistant response could not be persisted; interaction remains incomplete and memory capture is suppressed");
        return;
    }

    if let Err(error) = Box::pin(modules.expression.observe_committed_response(
        &modules.context,
        &pending.context,
        pending.response_id,
    ))
    .await
    {
        tracing::debug!(response_id=%pending.response_id,error=%error,"committed response appraisal was not admitted");
    }

    if let Some(entry) = cache_entry_for_pending(&pending) {
        if let Err(error) = modules.cache.put(entry).await {
            tracing::debug!(?error, "pending cache write was not admitted");
        }
    }

    let capture_trace = if pending.user_id.is_nil() {
        MemoryCaptureTrace::skipped(memory_config.capture_mode, "anonymous_user")
    } else {
        capture_turn_memory(
            &modules.context,
            modules
                .memory_capture_llm
                .as_deref()
                .unwrap_or(modules.minor_llm.as_ref()),
            &modules.memory,
            &CaptureTurn {
                context: pending.context.clone(),
                user_id: pending.user_id,
                user_id_text: pending.user_id_text.clone(),
                channel_id: pending.channel_id.clone(),
                session_id: pending.session_id,
                query_text: pending.query_text.clone(),
                response_text: pending.response_text.clone(),
                source_privacy_epoch: pending.source_privacy_epoch,
            },
            memory_config,
        )
        .await
    };
    record_memory_capture_trace(modules, &pending, capture_trace, trace).await;

    event_bus.publish(ServerEvent::ConversationTurnCompleted {
        channel_id: completed_channel_id,
        user_id: completed_user_id,
        user_uuid: completed_user_uuid,
        session_id: completed_session_id,
    });
}

pub(super) async fn persist_interrupted_user_context(
    modules: &InteractionServices,
    pending: &PendingTextPersistence,
    turn: &VoiceTurn,
    debug_output: bool,
) {
    delivery::interrupt_response(modules, turn, pending.user_id).await;
    let interruption = turn.interruption();
    let context = InterruptedUserContext {
        session_id: pending.session_id,
        source_event_id: pending.source_event_id,
        turn_id: turn.id,
        user_id: pending.user_id,
        user_id_text: pending.user_id_text.clone(),
        query_text: pending.query_text.clone(),
        channel_id: pending.channel_id.clone(),
        reason: interruption.reason,
        stage: interruption.stage,
    };
    persist_interrupted_user_context_entry(modules, context, debug_output).await;
}

pub(super) async fn persist_interrupted_input_context(
    modules: &InteractionServices,
    session_id: Uuid,
    user_id: Uuid,
    user_id_text: &str,
    query_text: &str,
    channel_id: &str,
    turn: &VoiceTurn,
    debug_output: bool,
) {
    delivery::interrupt_response(modules, turn, user_id).await;
    let interruption = turn.interruption();
    let context = InterruptedUserContext {
        session_id,
        source_event_id: turn.source_event_id.get().copied(),
        turn_id: turn.id,
        user_id,
        user_id_text: user_id_text.to_string(),
        query_text: query_text.to_string(),
        channel_id: channel_id.to_string(),
        reason: interruption.reason,
        stage: interruption.stage,
    };
    persist_interrupted_user_context_entry(modules, context, debug_output).await;
}

pub(super) async fn persist_interrupted_user_context_entry(
    modules: &InteractionServices,
    context: InterruptedUserContext,
    debug_output: bool,
) {
    let preserved = if context.source_event_id.is_some() {
        true
    } else {
        modules
            .conversation
            .append_message(
                context.session_id,
                ChatMessage {
                    role: "user".into(),
                    content: context.query_text.clone(),
                    name: Some(context.user_id_text.clone()),
                    ..ChatMessage::default()
                },
            )
            .await
            .is_ok()
    };

    let event = ConversationEventEntry {
        turn_id: context.turn_id,
        // Terminal interruption shares the response audit ID; reserve the last
        // sequence so it cannot collide with normal events or TTS (10_000+).
        sequence: i32::MAX,
        event_type: "turn_interrupted".to_string(),
        actor: "pipeline".to_string(),
        summary: Some("Voice turn interrupted; preserved user context only".to_string()),
        payload: serde_json::json!({
            "channel_id": context.channel_id.as_str(),
            "user_id": context.user_id_text.as_str(),
            "user_uuid": context.user_id.to_string(),
            "reason": context.reason.as_str(),
            "stage": context.stage,
            "preserved_user_context": preserved,
            "assistant_response_persisted": false,
            "cache_written": false,
            "memory_capture": false,
        }),
    };
    if let Err(error) = modules
        .conversation
        .append_event(context.session_id, event)
        .await
    {
        tracing::debug!(error = ?error, "failed to append interrupted turn event");
    }

    debug_component_json(
        debug_output,
        "pipeline",
        "turn_interrupted",
        &serde_json::json!({
            "channel_id": context.channel_id.as_str(),
            "user_id": context.user_id_text.as_str(),
            "reason": context.reason.as_str(),
            "stage": context.stage,
            "preserved_user_context": preserved,
        }),
        Some((&context.channel_id, &context.user_id_text)),
    );
}

pub(super) async fn record_memory_capture_trace(
    modules: &InteractionServices,
    pending: &PendingTextPersistence,
    capture_trace: MemoryCaptureTrace,
    trace: Option<&mut AgentTrace>,
) {
    let status = format!("{:?}", capture_trace.status).to_ascii_lowercase();
    let summary = match capture_trace.reason.as_deref() {
        Some(reason) => format!("Memory capture {status}: {reason}"),
        None => format!(
            "Memory capture {status}: {} stored, {} skipped",
            capture_trace.stored, capture_trace.skipped
        ),
    };
    let payload = serde_json::json!({
        "memory_capture": capture_trace,
        "context": {
            "channel_id": pending.channel_id,
            "conversation_id": pending.session_id,
            "user_id_text": pending.user_id_text,
        }
    });
    if let Some(trace) = trace {
        trace
            .record(modules, "memory_capture", "memory", Some(summary), payload)
            .await;
        return;
    }
    let event = ConversationEventEntry {
        turn_id: Uuid::new_v4(),
        sequence: 0,
        event_type: "memory_capture".to_string(),
        actor: "memory".to_string(),
        summary: Some(summary),
        payload,
    };
    if let Err(error) = modules
        .conversation
        .append_event(pending.session_id, event)
        .await
    {
        tracing::debug!(
            error = ?error,
            "failed to append standalone memory capture event"
        );
    }
}

pub fn cache_entry_for_pending(pending: &PendingTextPersistence) -> Option<CacheEntry> {
    let cache_write = pending.cache_write.as_ref()?;
    Some(CacheEntry {
        context: pending.context.clone(),
        origin_response_id: pending.response_id,
        scope: cache_write.scope.clone(),
        query_text: pending.query_text.clone(),
        response_text: pending.response_text.clone(),
        metadata: cache_write.metadata.clone(),
        ttl_seconds: cache_write.ttl_seconds,
    })
}
