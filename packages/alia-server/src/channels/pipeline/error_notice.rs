use serde::Serialize;
use uuid::Uuid;

use super::debug::{debug_component_json, log_channel_delivery_failure};
use super::turn::{VoiceTurn, cancellable_stage, turn_is_interrupted};
use crate::channels::adapter::{AudioFormat, AudioPayload, ChannelAdapter, ChannelTarget};
use crate::module::ModuleRegistry;
use crate::modules::memory::context::{ContextSource, SourceNotice};
use crate::providers::adapter::{ConversationEventEntry, TtsRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PipelineErrorStage {
    Stt,
    TranscriptCorrection,
    Llm,
    Tts,
    Delivery,
    Unknown,
}

impl PipelineErrorStage {
    pub(super) fn code(self) -> &'static str {
        match self {
            PipelineErrorStage::Stt => "stt",
            PipelineErrorStage::TranscriptCorrection => "transcript_correction",
            PipelineErrorStage::Llm => "llm",
            PipelineErrorStage::Tts => "tts",
            PipelineErrorStage::Delivery => "delivery",
            PipelineErrorStage::Unknown => "unknown",
        }
    }

    fn zh_summary(self) -> &'static str {
        match self {
            PipelineErrorStage::Stt => "语音识别没有完成",
            PipelineErrorStage::TranscriptCorrection => "语音文本校正没有完成",
            PipelineErrorStage::Llm => "生成回复时出错了",
            PipelineErrorStage::Tts => "语音合成没有完成",
            PipelineErrorStage::Delivery => "回复发送没有完成",
            PipelineErrorStage::Unknown => "处理请求时出错了",
        }
    }

    fn en_summary(self) -> &'static str {
        match self {
            PipelineErrorStage::Stt => "speech recognition did not complete",
            PipelineErrorStage::TranscriptCorrection => {
                "speech transcript correction did not complete"
            }
            PipelineErrorStage::Llm => "generating the reply failed",
            PipelineErrorStage::Tts => "speech synthesis did not complete",
            PipelineErrorStage::Delivery => "delivering the reply did not complete",
            PipelineErrorStage::Unknown => "processing the request failed",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct PipelineErrorNotice {
    pub error_id: String,
    pub stage: PipelineErrorStage,
    pub safe_summary: String,
    pub user_text: String,
    pub tts_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PipelineErrorDelivery {
    Text,
    Tts,
    DirectResponse,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct PipelineErrorNotification {
    pub notice: PipelineErrorNotice,
    pub delivery: PipelineErrorDelivery,
}

pub(super) async fn notify_pipeline_error(
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    stage: PipelineErrorStage,
    user_input: Option<&str>,
    raw_error: Option<&anyhow::Error>,
    debug_output: bool,
    prefer_voice: bool,
    turn: Option<&VoiceTurn>,
) -> PipelineErrorNotification {
    if let Some(error) = raw_error {
        tracing::error!(
            channel_type = channel.channel_type(),
            channel_id,
            user_id,
            stage = stage.code(),
            error = %error,
            "pipeline stage failed; notifying user with safe summary"
        );
        tracing::debug!(
            channel_type = channel.channel_type(),
            channel_id,
            user_id,
            stage = stage.code(),
            error = ?error,
            "pipeline stage failure details"
        );
    }

    let notice = build_pipeline_error_notice(stage, user_input);
    let Some(original_turn) = turn else {
        return PipelineErrorNotification {
            notice,
            delivery: PipelineErrorDelivery::Failed,
        };
    };
    if turn_is_interrupted(turn) {
        return PipelineErrorNotification {
            notice,
            delivery: PipelineErrorDelivery::Cancelled,
        };
    }
    let notice_turn = async {
        let owner = Uuid::parse_str(user_id)?;
        super::delivery::interrupt_response(modules, original_turn, owner).await;
        let existing = original_turn.context.lock().await.generation.clone();
        let source = if let Some(context) = existing {
            context.source().clone()
        } else {
            let event = original_turn.source_event_id.get().copied();
            ContextSource {
                owner_id: owner,
                request_id: original_turn.id,
                channel_id: channel_id.into(),
                conversation_id: if event.is_some() {
                    Some(
                        modules
                            .conversation
                            .get_or_create_active_session(channel_id, user_id)
                            .await?,
                    )
                } else {
                    None
                },
                source_event_id: event,
                source_received_at: original_turn.received_at,
                source_privacy_epoch: original_turn
                    .source_privacy_epoch
                    .ok_or_else(|| anyhow::anyhow!("notice has no original source epoch"))?,
            }
        };
        let kind = if user_input.map(contains_cjk).unwrap_or(true) {
            SourceNotice::ProcessingFailedZh
        } else {
            SourceNotice::ProcessingFailedEn
        };
        let lease = modules.context.source_notice_for(source, kind).await?;
        Ok::<_, anyhow::Error>(original_turn.for_notice(lease))
    }
    .await;
    let notice_turn = match notice_turn {
        Ok(turn) => turn,
        Err(error) => {
            tracing::warn!(%error, "trusted error notice was not admitted");
            return PipelineErrorNotification {
                notice,
                delivery: PipelineErrorDelivery::Failed,
            };
        }
    };
    let turn = Some(&notice_turn);
    if !turn_is_interrupted(turn)
        && super::delivery::prepare_response(
            modules,
            turn,
            channel_id,
            user_id,
            None,
            "error_notice",
        )
        .await
        .is_err()
    {
        return PipelineErrorNotification {
            notice,
            delivery: PipelineErrorDelivery::Failed,
        };
    }
    debug_component_json(
        debug_output,
        "pipeline_error",
        "notice",
        &serde_json::json!({
            "error_id": notice.error_id,
            "stage": stage.code(),
            "summary": notice.safe_summary,
            "prefer_voice": prefer_voice,
        }),
        Some((channel_id, user_id)),
    );

    let delivery = if turn_is_interrupted(turn) {
        PipelineErrorDelivery::Cancelled
    } else if prefer_voice && channel.capabilities().voice_output {
        deliver_voice_error_notice(
            modules,
            channel,
            target,
            channel_id,
            user_id,
            &notice,
            debug_output,
            turn,
        )
        .await
    } else {
        deliver_text_error_notice(
            modules,
            channel,
            target,
            channel_id,
            user_id,
            &notice,
            debug_output,
            turn,
        )
        .await
    };

    record_pipeline_error_event(modules, channel_id, user_id, &notice, delivery, user_input).await;
    if let (Some(turn), Ok(owner)) = (turn, Uuid::parse_str(user_id)) {
        super::delivery::interrupt_response(modules, turn, owner).await;
    }

    PipelineErrorNotification { notice, delivery }
}

pub(super) async fn play_error_direct_response(
    _channel: &dyn ChannelAdapter,
    channel_id: &str,
    user_id: &str,
    reason: &str,
    _debug_output: bool,
    turn: Option<&VoiceTurn>,
) -> PipelineErrorDelivery {
    if turn_is_interrupted(turn) {
        return PipelineErrorDelivery::Cancelled;
    }
    tracing::warn!(
        channel_id,
        user_id,
        reason,
        "local error cue lacks an immutable playback identity; no unscoped output was dispatched"
    );
    PipelineErrorDelivery::Failed
}

pub(super) async fn record_pipeline_error_event(
    modules: &ModuleRegistry,
    channel_id: &str,
    user_id: &str,
    notice: &PipelineErrorNotice,
    delivery: PipelineErrorDelivery,
    user_input: Option<&str>,
) {
    let session_id = match modules
        .conversation
        .get_or_create_active_session(channel_id, user_id)
        .await
    {
        Ok(session_id) => session_id,
        Err(error) => {
            tracing::debug!(error = ?error, "failed to create session for pipeline error event");
            return;
        }
    };

    let event = ConversationEventEntry {
        turn_id: Uuid::new_v4(),
        sequence: 0,
        event_type: "pipeline_error_notified".to_string(),
        actor: "pipeline".to_string(),
        summary: Some(notice.safe_summary.clone()),
        payload: serde_json::json!({
            "error_id": notice.error_id,
            "stage": notice.stage.code(),
            "safe_summary": notice.safe_summary,
            "delivery": delivery,
            "user_input_preview": user_input.map(|input| preview(input, 240)),
        }),
    };
    if let Err(error) = modules.conversation.append_event(session_id, event).await {
        tracing::debug!(error = ?error, "failed to append pipeline error event");
    }
}

fn build_pipeline_error_notice(
    stage: PipelineErrorStage,
    user_input: Option<&str>,
) -> PipelineErrorNotice {
    let error_id = Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect::<String>();
    let chinese = user_input.map(contains_cjk).unwrap_or(true);
    let safe_summary = if chinese {
        stage.zh_summary().to_string()
    } else {
        stage.en_summary().to_string()
    };
    let literal = if chinese {
        SourceNotice::ProcessingFailedZh
    } else {
        SourceNotice::ProcessingFailedEn
    }
    .text();
    let user_text = literal.to_string();
    let tts_text = literal.to_string();

    PipelineErrorNotice {
        error_id,
        stage,
        safe_summary,
        user_text,
        tts_text,
    }
}

async fn deliver_text_error_notice(
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    notice: &PipelineErrorNotice,
    debug_output: bool,
    turn: Option<&VoiceTurn>,
) -> PipelineErrorDelivery {
    if turn_is_interrupted(turn) {
        return PipelineErrorDelivery::Cancelled;
    }
    if !channel.capabilities().text {
        return PipelineErrorDelivery::Failed;
    }
    debug_component_json(
        debug_output,
        "pipeline_error",
        "send_text",
        &serde_json::json!({
            "error_id": notice.error_id,
            "stage": notice.stage.code(),
        }),
        Some((channel_id, user_id)),
    );
    match cancellable_stage(
        turn,
        "error_text",
        super::delivery::text(
            modules,
            target,
            turn,
            &format!("text:error:{}", notice.error_id),
            &notice.user_text,
        ),
    )
    .await
    {
        Ok(None) => PipelineErrorDelivery::Cancelled,
        Ok(Some(record))
            if record.status == crate::modules::delivery::types::DeliveryStatus::Submitted =>
        {
            PipelineErrorDelivery::Text
        }
        Ok(Some(_)) => PipelineErrorDelivery::Failed,
        Err(error) => {
            log_channel_delivery_failure(channel, error).await;
            PipelineErrorDelivery::Failed
        }
    }
}

async fn deliver_voice_error_notice(
    modules: &ModuleRegistry,
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    notice: &PipelineErrorNotice,
    debug_output: bool,
    turn: Option<&VoiceTurn>,
) -> PipelineErrorDelivery {
    if notice.stage == PipelineErrorStage::Tts {
        return play_error_direct_response(
            channel,
            channel_id,
            user_id,
            "pipeline_error_tts_failed",
            debug_output,
            turn,
        )
        .await;
    }

    debug_component_json(
        debug_output,
        "pipeline_error",
        "tts_call",
        &serde_json::json!({
            "error_id": notice.error_id,
            "stage": notice.stage.code(),
        }),
        Some((channel_id, user_id)),
    );
    let budget =
        match super::streaming::prepare_audio_budget(modules, target, channel_id, turn).await {
            Ok(budget) => budget,
            Err(error) => {
                tracing::warn!(error=%error, "error TTS has no admitted Response budget");
                return PipelineErrorDelivery::Failed;
            }
        };
    let audio_bytes = match cancellable_stage(
        turn,
        "error_tts",
        modules
            .tts
            .synthesize_request(&TtsRequest::plain(&notice.tts_text), budget),
    )
    .await
    {
        Ok(None) => return PipelineErrorDelivery::Cancelled,
        Ok(Some(audio_bytes)) => audio_bytes,
        Err(error) => {
            tracing::warn!(
                channel_type = channel.channel_type(),
                channel_id = channel.channel_id(),
                error = %error,
                "TTS error notice failed; falling back to local direct response"
            );
            tracing::debug!(error = ?error, "TTS error notice failure details");
            return play_error_direct_response(
                channel,
                channel_id,
                user_id,
                "pipeline_error_tts_notice_failed",
                debug_output,
                turn,
            )
            .await;
        }
    };

    let audio_out = AudioPayload {
        data: audio_bytes.into_buffer(),
        sample_rate: 24000,
        channels: 1,
        format: AudioFormat::Wav,
    };
    match cancellable_stage(
        turn,
        "error_audio",
        super::delivery::audio(
            modules,
            target,
            turn,
            &format!("error:{}", notice.stage.code()),
            &notice.tts_text,
            Box::pin(futures::stream::iter([Ok(audio_out)])),
        ),
    )
    .await
    {
        Ok(None) => PipelineErrorDelivery::Cancelled,
        Ok(Some(record)) => match record.status {
            crate::modules::delivery::types::DeliveryStatus::Completed
            | crate::modules::delivery::types::DeliveryStatus::Submitted => {
                PipelineErrorDelivery::Tts
            }
            crate::modules::delivery::types::DeliveryStatus::Cancelled => {
                PipelineErrorDelivery::Cancelled
            }
            _ => PipelineErrorDelivery::Failed,
        },
        Err(error) => {
            log_channel_delivery_failure(channel, error).await;
            play_error_direct_response(
                channel,
                channel_id,
                user_id,
                "pipeline_error_audio_delivery_failed",
                debug_output,
                turn,
            )
            .await
        }
    }
}

fn contains_cjk(input: &str) -> bool {
    input.chars().any(|ch| {
        matches!(
            ch as u32,
            0x3400..=0x4DBF
                | 0x4E00..=0x9FFF
                | 0xF900..=0xFAFF
                | 0x3040..=0x30FF
                | 0xAC00..=0xD7AF
        )
    })
}

fn preview(input: &str, max_chars: usize) -> String {
    let mut out = input.chars().take(max_chars).collect::<String>();
    if input.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::adapter::ChannelAction;

    #[test]
    fn builds_chinese_notice_without_raw_error() {
        let notice = build_pipeline_error_notice(PipelineErrorStage::Llm, Some("介绍一下终末地"));

        assert_eq!(notice.user_text, SourceNotice::ProcessingFailedZh.text());
        assert_eq!(notice.tts_text, SourceNotice::ProcessingFailedZh.text());
        assert!(!notice.user_text.contains("connection refused"));
        assert_eq!(notice.error_id.len(), 8);
    }

    #[test]
    fn builds_english_notice_for_non_cjk_input() {
        let notice = build_pipeline_error_notice(PipelineErrorStage::Stt, Some("hello"));

        assert_eq!(notice.user_text, SourceNotice::ProcessingFailedEn.text());
        assert!(notice.safe_summary.contains("speech recognition"));
    }
    #[derive(Default)]
    struct RecordingChannel {
        effects: std::sync::atomic::AtomicUsize,
    }
    impl crate::module::ModuleLifecycle for RecordingChannel {
        fn name(&self) -> &'static str {
            "recording_channel"
        }
    }
    #[async_trait::async_trait]
    impl ChannelAdapter for RecordingChannel {
        fn channel_type(&self) -> &'static str {
            "test"
        }
        fn channel_id(&self) -> &str {
            "test"
        }
        fn capabilities(&self) -> crate::channels::adapter::ChannelCapabilities {
            crate::channels::adapter::ChannelCapabilities {
                voice_output: true,
                ..Default::default()
            }
        }
        async fn send_message(&self, _: &ChannelTarget, _: &str) -> anyhow::Result<()> {
            self.effects
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn send_audio(&self, _: &ChannelTarget, _: AudioPayload) -> anyhow::Result<()> {
            self.effects
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn execute_action(&self, _: ChannelAction) -> anyhow::Result<serde_json::Value> {
            self.effects
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(serde_json::json!({"played": true}))
        }
    }
    struct CancellingTts {
        cancel: tokio_util::sync::CancellationToken,
        fails: bool,
    }
    impl crate::module::ModuleLifecycle for CancellingTts {
        fn name(&self) -> &'static str {
            "cancelling_tts"
        }
    }
    #[async_trait::async_trait]
    impl crate::providers::adapter::TtsAdapter for CancellingTts {
        async fn synthesize_request(
            &self,
            _: &crate::providers::adapter::TtsRequest,
            budget: crate::providers::tts::audio_budget::ResponseAudioBudget,
        ) -> anyhow::Result<crate::providers::tts::audio_budget::EncodedAudio> {
            self.cancel.cancel();
            if self.fails {
                anyhow::bail!("test synthesis error");
            }
            budget.copy_from_slice(&[1, 2]).await
        }
        async fn synthesize_stream_request(
            &self,
            _: &crate::providers::adapter::TtsRequest,
            _: crate::providers::tts::audio_budget::ResponseAudioBudget,
        ) -> anyhow::Result<crate::providers::tts::audio_budget::TtsAudioStream> {
            unreachable!("fixture expects buffered synthesis")
        }
    }

    #[tokio::test]
    async fn unadmitted_error_notice_never_starts_tts_or_side_effects() {
        for fails in [false, true] {
            let turn = VoiceTurn::request(tokio_util::sync::CancellationToken::new());
            let channel = RecordingChannel::default();
            let tts = CancellingTts {
                cancel: turn.token.clone(),
                fails,
            };
            let target = ChannelTarget {
                voice: None,
                user_id: "user".into(),
            };
            let notice = build_pipeline_error_notice(PipelineErrorStage::Llm, Some("hello"));
            let mut fixture = super::super::integration_tests::Fixture::new(false).await;
            std::sync::Arc::get_mut(&mut fixture.modules).unwrap().tts = std::sync::Arc::new(tts);
            let delivery = deliver_voice_error_notice(
                &fixture.modules,
                &channel,
                &target,
                "test",
                "user",
                &notice,
                false,
                Some(&turn),
            )
            .await;
            assert_eq!(delivery, PipelineErrorDelivery::Failed);
            assert!(
                !turn.token.is_cancelled(),
                "TTS must not run before Response preparation"
            );
            assert_eq!(channel.effects.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn cancelled_error_does_not_dispatch_direct_response_or_text() {
        let fixture = super::super::integration_tests::Fixture::new(false).await;
        let turn = VoiceTurn::request(tokio_util::sync::CancellationToken::new());
        turn.token.cancel();
        let channel = RecordingChannel::default();
        let target = ChannelTarget {
            voice: None,
            user_id: "user".into(),
        };
        let notice = build_pipeline_error_notice(PipelineErrorStage::Tts, None);
        assert_eq!(
            play_error_direct_response(&channel, "test", "user", "failure", false, Some(&turn))
                .await,
            PipelineErrorDelivery::Cancelled
        );
        assert_eq!(
            deliver_text_error_notice(
                &fixture.modules,
                &channel,
                &target,
                "test",
                "user",
                &notice,
                false,
                Some(&turn)
            )
            .await,
            PipelineErrorDelivery::Cancelled
        );
        assert_eq!(channel.effects.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}
