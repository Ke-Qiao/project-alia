use super::services::InteractionServices;
use super::turn::VoiceTurn;
use crate::modules::memory::context::{ContextSource, ContextUse, GenerationContext};
use crate::providers::adapter::{CachedResponse, ChatMessage};
use uuid::Uuid;

pub(super) async fn ensure_turn_context(
    modules: &InteractionServices,
    turn: &VoiceTurn,
    channel_id: &str,
    user_id: &str,
    raw_text: &str,
) -> anyhow::Result<GenerationContext> {
    let mut state = turn.context.lock().await;
    if let Some(context) = &state.generation {
        return Ok(context.clone());
    }
    anyhow::ensure!(
        !state.frozen && state.notice.is_none(),
        "turn context is already fixed"
    );
    let owner_id = Uuid::parse_str(user_id)?;
    let epoch = turn
        .source_privacy_epoch
        .ok_or_else(|| anyhow::anyhow!("input has no trusted source privacy epoch"))?;
    let relationship_epoch = turn
        .source_relationship_epoch
        .ok_or_else(|| anyhow::anyhow!("input has no trusted source relationship epoch"))?;
    anyhow::ensure!(
        epoch >= 0 && relationship_epoch >= 0 && !turn.token.is_cancelled(),
        "input is no longer admitted"
    );
    let session_id = modules
        .conversation
        .get_or_create_active_session(channel_id, user_id)
        .await?;
    let source_event_id = match turn.source_event_id.get() {
        Some(id) => *id,
        None => {
            let id = modules
                .conversation
                .append_sourced_message(
                    owner_id,
                    session_id,
                    ChatMessage::new("user", raw_text),
                    turn.received_at,
                    epoch,
                    relationship_epoch,
                )
                .await?;
            turn.source_event_id
                .set(id)
                .map_err(|_| anyhow::anyhow!("turn acquired a different user source"))?;
            id
        }
    };
    let context = modules
        .context
        .materialize(
            ContextSource {
                owner_id,
                request_id: turn.id,
                channel_id: channel_id.to_string(),
                conversation_id: Some(session_id),
                source_event_id: Some(source_event_id),
                source_received_at: turn.received_at,
                source_privacy_epoch: epoch,
            },
            raw_text,
        )
        .await?;
    state.generation = Some(context.clone());
    Ok(context)
}

pub(super) async fn validate_generation_context(
    modules: &InteractionServices,
    turn: Option<&VoiceTurn>,
) -> anyhow::Result<()> {
    let generation = turn
        .ok_or_else(|| anyhow::anyhow!("generation has no admitted turn"))?
        .generation_context()
        .await?;
    modules
        .context
        .validate(&generation.lease().binding(), ContextUse::ResponseCommit)
        .await?;
    Ok(())
}

pub(super) async fn merge_turn_cache(
    modules: &InteractionServices,
    turn: &VoiceTurn,
    cached: &CachedResponse,
) -> anyhow::Result<()> {
    let mut state = turn.context.lock().await;
    anyhow::ensure!(
        !state.frozen,
        "cannot add cache evidence after output preparation"
    );
    let original = state
        .generation
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("cache reuse has no original context"))?;
    let merged = modules
        .context
        .merge_cache(original, cached.context_identity())
        .await?;
    state.generation = Some(merged);
    Ok(())
}
