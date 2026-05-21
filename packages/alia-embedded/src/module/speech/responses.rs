use std::sync::Arc;

use crate::module::audio_debug::AudioDebugRecorder;
use crate::module::context::EventBus;
use crate::module::direct_response_playback;
use crate::module::traits::{AudioPlaybackBackend, DirectResponseAction, DirectResponseEngine};

pub use crate::module::direct_response_playback::{pcm_duration, read_pcm16_wav_blocking};

pub(super) const ACTION_WAKEWORD: &str = "wakeword";
pub(super) const ACTION_FALLBACK: &str = "fallback";
pub(super) const ACTION_CANCEL: &str = "cancel";
pub(super) const ACTION_LISTENING_START: &str = "listening_start";
pub(super) const ACTION_LISTENING_CONTINUE: &str = "listening_continue";
pub(super) const ACTION_THINKING: &str = "thinking";

pub(super) async fn play_direct_response(
    action: DirectResponseAction,
    playback: Arc<dyn AudioPlaybackBackend>,
    event_bus: EventBus,
    audio_debug: AudioDebugRecorder,
) -> bool {
    direct_response_playback::play_direct_response(action, playback, event_bus, audio_debug).await
}

pub(super) async fn play_direct_response_action(
    action: &str,
    direct_response: Arc<dyn DirectResponseEngine>,
    playback: Arc<dyn AudioPlaybackBackend>,
    event_bus: EventBus,
    audio_debug: AudioDebugRecorder,
) -> bool {
    direct_response_playback::play_direct_response_action(
        action,
        direct_response,
        playback,
        event_bus,
        audio_debug,
    )
    .await
}

pub(super) async fn play_direct_response_action_or_keyword(
    action: &str,
    keyword: &str,
    direct_response: Arc<dyn DirectResponseEngine>,
    playback: Arc<dyn AudioPlaybackBackend>,
    event_bus: EventBus,
    audio_debug: AudioDebugRecorder,
) -> bool {
    direct_response_playback::play_direct_response_action_or_keyword(
        action,
        keyword,
        direct_response,
        playback,
        event_bus,
        audio_debug,
    )
    .await
}

pub(super) fn is_cancel_direct_response(action: &DirectResponseAction) -> bool {
    action
        .action
        .as_deref()
        .is_some_and(|name| normalize_response_action(name) == ACTION_CANCEL)
}

fn normalize_response_action(input: &str) -> String {
    input
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|ch| if ch == '-' { '_' } else { ch })
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect()
}
