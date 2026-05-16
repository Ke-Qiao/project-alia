use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::binding::BindingManager;
use crate::module::audio_debug::AudioDebugRecorder;
use crate::module::context::EventBus;
use crate::module::traits::{AudioPlaybackBackend, DirectResponseEngine, EdgeSttEngine};

use super::capture::SpeechCapture;
use super::responses::{
    ACTION_FALLBACK, is_cancel_direct_response, play_direct_response, play_direct_response_action,
};
use super::transport::upload_to_server;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UtteranceOutcome {
    Ignored,
    UploadedToServer,
    LocalDirectResponse,
    FallbackResponse,
    Cancelled,
}

pub(super) async fn handle_utterance(
    capture: SpeechCapture,
    edge_stt: Arc<dyn EdgeSttEngine>,
    direct_response: Arc<dyn DirectResponseEngine>,
    playback: Arc<dyn AudioPlaybackBackend>,
    binding: Arc<BindingManager>,
    audio_debug: AudioDebugRecorder,
    event_bus: EventBus,
    max_duration_ms: u32,
) -> UtteranceOutcome {
    if capture.pcm.is_empty() {
        return UtteranceOutcome::Ignored;
    }
    debug!(
        origin = capture.origin.as_str(),
        samples = capture.pcm.len(),
        sample_rate = capture.sample_rate,
        "handling captured utterance"
    );
    audio_debug.record_wake_capture(&capture.pcm, capture.sample_rate, &capture.wakeword.keyword);

    let transcript = match edge_stt
        .transcribe_short(&capture.pcm, max_duration_ms)
        .await
    {
        Ok(Some(text)) if !text.trim().is_empty() => {
            info!("edge STT produced transcript");
            debug!(transcript = %text, "edge STT transcript details");
            text
        }
        Ok(_) => {
            debug!(keyword = %capture.wakeword.keyword, "edge STT produced no transcript");
            return upload_or_play_fallback(
                &capture,
                &binding,
                direct_response,
                playback,
                event_bus,
                audio_debug,
            )
            .await;
        }
        Err(error) => {
            warn!(error = %error, "edge STT failed");
            return upload_or_play_fallback(
                &capture,
                &binding,
                direct_response,
                playback,
                event_bus,
                audio_debug,
            )
            .await;
        }
    };

    let Some(action) = direct_response.match_keyword(&transcript) else {
        debug!(%transcript, "no direct response keyword matched");
        return upload_or_play_fallback(
            &capture,
            &binding,
            direct_response,
            playback,
            event_bus,
            audio_debug,
        )
        .await;
    };

    let is_cancel = is_cancel_direct_response(&action);
    play_direct_response(action, playback, event_bus, audio_debug).await;
    if is_cancel {
        UtteranceOutcome::Cancelled
    } else {
        UtteranceOutcome::LocalDirectResponse
    }
}

async fn upload_or_play_fallback(
    capture: &SpeechCapture,
    binding: &BindingManager,
    direct_response: Arc<dyn DirectResponseEngine>,
    playback: Arc<dyn AudioPlaybackBackend>,
    event_bus: EventBus,
    audio_debug: AudioDebugRecorder,
) -> UtteranceOutcome {
    if upload_to_server(capture, binding).await {
        return UtteranceOutcome::UploadedToServer;
    }

    play_direct_response_action(
        ACTION_FALLBACK,
        direct_response,
        playback,
        event_bus,
        audio_debug,
    )
    .await;
    UtteranceOutcome::FallbackResponse
}
