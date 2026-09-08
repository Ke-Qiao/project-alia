use super::turn::VoiceTurn;
use crate::{
    channels::adapter::{AudioPayloadStream, ChannelTarget},
    module::ModuleRegistry,
    modules::delivery::{
        AudioDelivery,
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
    if let Some(id) = turn.response_id.get() {
        return Ok(*id);
    }
    let response=modules.delivery.prepare(NewResponse{key:ResponseKey{user_id:owner,request_id:turn.id,output_key:"response".into()},conversation_id,source_event_id:turn.source_event_id.get().copied(),source_received_at:turn.received_at,source_privacy_epoch:turn.source_privacy_epoch,channel_id:channel_id.into(),kind:kind.into(),content:json!({"kind":kind,"source_event_id":turn.source_event_id.get(),"voice_scope":turn.voice_scope})}).await?;
    if let Err(id) = turn.response_id.set(response.response_id) {
        anyhow::ensure!(
            turn.response_id.get() == Some(&id),
            "turn acquired different response identity"
        );
    }
    Ok(response.response_id)
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
