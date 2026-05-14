pub(crate) mod adapter;
pub(crate) mod cli;
#[cfg(feature = "channel-embedded")]
pub(crate) mod embedded;
pub(crate) mod pipeline;
pub(crate) mod registry;

use crate::providers::adapter::TtsAdapter;

pub use adapter::ChannelAdapter;
pub use pipeline::spawn_pipeline;
pub use registry::ChannelRegistry;

pub(crate) async fn ensure_default_wakeword_response(
    channel: &dyn ChannelAdapter,
    tts: &dyn TtsAdapter,
) {
    let Ok(list) = channel
        .execute_action(adapter::ChannelAction {
            name: "list_direct_responses".to_string(),
            params: serde_json::json!({}),
        })
        .await
    else {
        return;
    };

    let has_wakeword_response = list
        .get("rules")
        .and_then(serde_json::Value::as_array)
        .map(|rules| {
            rules.iter().any(|rule| {
                rule.get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true)
                    && rule
                        .get("action")
                        .and_then(serde_json::Value::as_str)
                        .map(|action| action == "wakeword")
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if has_wakeword_response {
        return;
    }

    let settings = channel
        .execute_action(adapter::ChannelAction {
            name: "get_settings".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap_or_else(|_| serde_json::json!({}));
    let text = settings
        .pointer("/settings/direct_response_defaults/default_wakeword_response_content")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Hello! How can I help you?");

    let audio = match tts.synthesize(text).await {
        Ok(audio) if !audio.is_empty() => audio,
        Ok(_) => {
            tracing::warn!("default wakeword direct response TTS returned empty audio");
            return;
        }
        Err(error) => {
            tracing::warn!(
                error = ?error,
                "failed to generate default wakeword direct response audio"
            );
            return;
        }
    };

    use base64::Engine;
    let audio_base64 = base64::engine::general_purpose::STANDARD.encode(audio);
    if let Err(error) = channel
        .execute_action(adapter::ChannelAction {
            name: "upsert_direct_response".to_string(),
            params: serde_json::json!({
                "keywords": ["hey alia", "你好阿利亚"],
                "action": "wakeword",
                "enabled": true,
                "audio_variants": [{
                    "audio_path": "wakeword-default.wav",
                    "weight": 1.0,
                    "audio_base64": audio_base64,
                }],
            }),
        })
        .await
    {
        tracing::warn!(
            error = ?error,
            "failed to store default wakeword direct response on embedded channel"
        );
    }
}
