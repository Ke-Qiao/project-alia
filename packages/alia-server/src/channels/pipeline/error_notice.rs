use serde::Serialize;
use uuid::Uuid;

use super::debug::{debug_component_json, log_channel_delivery_failure};
use crate::channels::adapter::{
    AudioFormat, AudioPayload, ChannelAction, ChannelAdapter, ChannelTarget,
};
use crate::module::ModuleRegistry;
use crate::providers::adapter::{ConversationEventEntry, TtsRequest};

const ERROR_DIRECT_RESPONSE_ACTION: &str = "error";

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

    let delivery = if prefer_voice && channel.capabilities().voice_output {
        deliver_voice_error_notice(
            modules,
            channel,
            target,
            channel_id,
            user_id,
            &notice,
            debug_output,
        )
        .await
    } else {
        deliver_text_error_notice(channel, target, channel_id, user_id, &notice, debug_output).await
    };

    record_pipeline_error_event(modules, channel_id, user_id, &notice, delivery, user_input).await;

    PipelineErrorNotification { notice, delivery }
}

pub(super) async fn play_error_direct_response(
    channel: &dyn ChannelAdapter,
    channel_id: &str,
    user_id: &str,
    reason: &str,
    debug_output: bool,
) -> PipelineErrorDelivery {
    debug_component_json(
        debug_output,
        "pipeline_error",
        "direct_response",
        &serde_json::json!({
            "action": ERROR_DIRECT_RESPONSE_ACTION,
            "reason": reason,
        }),
        Some((channel_id, user_id)),
    );

    match channel
        .execute_action(ChannelAction {
            name: "play_direct_response".to_string(),
            params: serde_json::json!({
                "action": ERROR_DIRECT_RESPONSE_ACTION,
                "interrupt_current": true,
                "reason": reason,
            }),
        })
        .await
    {
        Ok(value) => {
            let played = value
                .get("played")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if played {
                PipelineErrorDelivery::DirectResponse
            } else {
                tracing::warn!(
                    channel_type = channel.channel_type(),
                    channel_id = channel.channel_id(),
                    response = %value,
                    "channel accepted error direct response but did not play audio"
                );
                PipelineErrorDelivery::Failed
            }
        }
        Err(error) => {
            tracing::debug!(
                channel_type = channel.channel_type(),
                channel_id = channel.channel_id(),
                error = ?error,
                "channel error direct response is unavailable"
            );
            PipelineErrorDelivery::Failed
        }
    }
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
    let user_text = if chinese {
        format!("抱歉，{safe_summary}。请再试一次。错误编号：{error_id}。")
    } else {
        format!("Sorry, {safe_summary}. Please try again. Error ID: {error_id}.")
    };
    let tts_text = if chinese {
        format!("抱歉，{safe_summary}。请再试一次。")
    } else {
        format!("Sorry, {safe_summary}. Please try again.")
    };

    PipelineErrorNotice {
        error_id,
        stage,
        safe_summary,
        user_text,
        tts_text,
    }
}

async fn deliver_text_error_notice(
    channel: &dyn ChannelAdapter,
    target: &ChannelTarget,
    channel_id: &str,
    user_id: &str,
    notice: &PipelineErrorNotice,
    debug_output: bool,
) -> PipelineErrorDelivery {
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
    match channel.send_message(target, &notice.user_text).await {
        Ok(()) => PipelineErrorDelivery::Text,
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
) -> PipelineErrorDelivery {
    if notice.stage == PipelineErrorStage::Tts {
        return play_error_direct_response(
            channel,
            channel_id,
            user_id,
            "pipeline_error_tts_failed",
            debug_output,
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
    let audio_bytes = match modules
        .tts
        .synthesize_request(&TtsRequest::plain(&notice.tts_text))
        .await
    {
        Ok(audio_bytes) => audio_bytes,
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
            )
            .await;
        }
    };

    let audio_out = AudioPayload {
        data: audio_bytes,
        sample_rate: 24000,
        channels: 1,
        format: AudioFormat::Wav,
    };
    match channel.send_audio(target, audio_out).await {
        Ok(()) => PipelineErrorDelivery::Tts,
        Err(error) => {
            log_channel_delivery_failure(channel, error).await;
            play_error_direct_response(
                channel,
                channel_id,
                user_id,
                "pipeline_error_audio_delivery_failed",
                debug_output,
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

    #[test]
    fn builds_chinese_notice_without_raw_error() {
        let notice = build_pipeline_error_notice(PipelineErrorStage::Llm, Some("介绍一下终末地"));

        assert!(notice.user_text.contains("生成回复"));
        assert!(notice.tts_text.contains("请再试一次"));
        assert!(!notice.user_text.contains("connection refused"));
        assert_eq!(notice.error_id.len(), 8);
    }

    #[test]
    fn builds_english_notice_for_non_cjk_input() {
        let notice = build_pipeline_error_notice(PipelineErrorStage::Stt, Some("hello"));

        assert!(notice.user_text.starts_with("Sorry,"));
        assert!(notice.safe_summary.contains("speech recognition"));
    }
}
