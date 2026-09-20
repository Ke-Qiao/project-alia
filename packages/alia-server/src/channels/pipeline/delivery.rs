use super::turn::VoiceTurn;
use crate::{
    channels::adapter::{AudioPayloadStream, ChannelTarget, TextPayloadStream},
    module::ModuleRegistry,
    modules::delivery::{
        AudioDelivery, TextDelivery,
        types::{DeliveryRecord, NewResponse, ResponseKey},
    },
};
use serde_json::json;
use uuid::Uuid;

pub(super) async fn prepare_response(
    modules: &ModuleRegistry,
    turn: Option<&VoiceTurn>,
    channel_id: &str,
    user_id: &str,
    conversation_id: Option<Uuid>,
    kind: &str,
) -> anyhow::Result<Uuid> {
    let turn = turn.ok_or_else(|| anyhow::anyhow!("output has no admitted turn owner"))?;
    let owner = Uuid::parse_str(user_id)
        .map_err(|_| anyhow::anyhow!("identity_unlinked: output requires a real account"))?;
    let mut state = turn.context.lock().await;
    if let Some(id) = turn.response_id.get() {
        return Ok(*id);
    }
    Box::pin(super::project_turn_affect(modules, &mut state)).await?;
    let lease = state.lease()?.clone();
    let source = lease.source();
    anyhow::ensure!(
        source.owner_id == owner
            && source.request_id == turn.id
            && source.channel_id == channel_id
            && source.source_event_id == turn.source_event_id.get().copied()
            && Some(source.source_privacy_epoch) == turn.source_privacy_epoch,
        "output differs from original context source"
    );
    if let Some(session) = conversation_id {
        anyhow::ensure!(
            source.conversation_id == Some(session),
            "output changed conversation"
        );
    }
    // Once admission has been attempted, even an unknown database acknowledgement
    // must be retried with this immutable context rather than a fresh lease.
    state.frozen = true;
    let notice_text = lease.notice_text();
    let kind = if notice_text.is_some() {
        "source_notice"
    } else {
        kind
    };
    let response = modules
        .delivery
        .prepare(NewResponse {
            key: ResponseKey {
                user_id: owner,
                request_id: turn.id,
                output_key: if notice_text.is_some() {
                    "trusted_notice"
                } else {
                    "response"
                }
                .into(),
            },
            context_id: lease.id(),
            conversation_id: source.conversation_id,
            source_event_id: source.source_event_id,
            source_received_at: source.source_received_at,
            source_privacy_epoch: Some(source.source_privacy_epoch),
            channel_id: channel_id.into(),
            kind: kind.into(),
            content: json!({"kind":kind,"source_event_id":source.source_event_id,
            "voice_scope":turn.voice_scope,"text":notice_text}),
        })
        .await?;
    if let Err(id) = turn.response_id.set(response.response_id) {
        anyhow::ensure!(
            turn.response_id.get() == Some(&id),
            "turn acquired different response identity"
        );
    }
    Ok(response.response_id)
}

pub(super) async fn text_stream(
    modules: &ModuleRegistry,
    target: &ChannelTarget,
    turn: Option<&VoiceTurn>,
    output_key: &str,
    content: serde_json::Value,
    text: TextPayloadStream,
) -> anyhow::Result<DeliveryRecord> {
    let turn = turn.ok_or_else(|| anyhow::anyhow!("text has no original turn owner"))?;
    let channel = turn
        .output_channel
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("text has no pinned original channel"))?
        .clone();
    anyhow::ensure!(
        channel.capabilities().text,
        "channel does not accept text delivery"
    );
    let notice = turn.context.lock().await.lease()?.notice_text();
    let text = if let Some(literal) = notice {
        use alia_core::text_delivery::TextFrame;
        use futures::StreamExt;
        let mut next = 0u8;
        Box::pin(text.map(move |frame| {
            let frame = frame?;
            let valid = match &frame {
                TextFrame::Chunk(chunk) => {
                    next == 0
                        && chunk.sequence == 0
                        && chunk.utf8_offset == 0
                        && chunk.text == literal
                }
                TextFrame::End { chunks, utf8_bytes } => {
                    next == 1 && *chunks == 1 && *utf8_bytes == literal.len() as u64
                }
            };
            anyhow::ensure!(valid, "notice stream differs from its trusted literal");
            next += 1;
            Ok(frame)
        })) as TextPayloadStream
    } else {
        text
    };
    let response_id = prepare_response(
        modules,
        Some(turn),
        channel.channel_id(),
        &target.user_id,
        None,
        "notice",
    )
    .await?;
    Ok(modules
        .delivery
        .deliver_text(TextDelivery {
            response_id,
            output_key: output_key.into(),
            content,
            channel,
            target: target.clone(),
            text,
            cancel: turn.token.clone(),
        })
        .await?)
}

/// A non-streaming model result uses exactly the same immutable chunk ledger
/// and explicit producer End as a streamed result.
pub(super) async fn text(
    modules: &ModuleRegistry,
    target: &ChannelTarget,
    turn: Option<&VoiceTurn>,
    output_key: &str,
    text: &str,
) -> anyhow::Result<DeliveryRecord> {
    use alia_core::text_delivery::{MAX_TEXT_BYTES, MAX_TEXT_CHUNK_BYTES, TextChunk, TextFrame};
    anyhow::ensure!(
        text.len() as u64 <= MAX_TEXT_BYTES,
        "text exceeds 256 KiB limit"
    );
    let mut frames = Vec::new();
    let mut remaining = text;
    let mut offset = 0;
    while !remaining.is_empty() {
        let mut boundary = remaining.len().min(MAX_TEXT_CHUNK_BYTES);
        while !remaining.is_char_boundary(boundary) {
            boundary -= 1;
        }
        frames.push(Ok(TextFrame::Chunk(TextChunk {
            sequence: frames.len() as u64,
            utf8_offset: offset,
            text: remaining[..boundary].into(),
        })));
        offset += boundary as u64;
        remaining = &remaining[boundary..];
    }
    frames.push(Ok(TextFrame::End {
        chunks: frames.len() as u64,
        utf8_bytes: offset,
    }));
    text_stream(
        modules,
        target,
        turn,
        output_key,
        json!({"representation":"plain_utf8","text":text}),
        Box::pin(futures::stream::iter(frames)),
    )
    .await
}

pub(super) async fn audio(
    modules: &ModuleRegistry,
    target: &ChannelTarget,
    turn: Option<&VoiceTurn>,
    output_key: &str,
    text: &str,
    audio: AudioPayloadStream,
) -> anyhow::Result<DeliveryRecord> {
    let turn = turn.ok_or_else(|| anyhow::anyhow!("audio has no original turn owner"))?;
    let channel = turn
        .output_channel
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("audio has no pinned original channel"))?
        .clone();
    anyhow::ensure!(
        target.voice == turn.voice_scope,
        "audio target differs from original voice scope"
    );
    if let Some(literal) = turn.context.lock().await.lease()?.notice_text() {
        anyhow::ensure!(
            text == literal,
            "notice audio differs from its trusted literal"
        );
    }
    let response_id = prepare_response(
        modules,
        Some(turn),
        channel.channel_id(),
        &target.user_id,
        None,
        "notice",
    )
    .await?;
    Ok(modules
        .delivery
        .deliver_audio(AudioDelivery {
            response_id,
            output_key: output_key.into(),
            content: json!({"text":text}),
            channel,
            target: target.clone(),
            audio,
            cancel: turn.token.clone(),
        })
        .await?)
}

pub(super) async fn interrupt_response(modules: &ModuleRegistry, turn: &VoiceTurn, owner: Uuid) {
    if let Some(response_id) = turn.response_id.get()
        && let Err(error) = modules.delivery.cancel_response(owner, *response_id).await
    {
        tracing::warn!(%response_id, %error, "interrupted response cancellation could not be persisted");
    }
}
