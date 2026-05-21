use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::channels::adapter::ChannelInterruptConfig;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct VoiceTurnKey {
    channel_id: String,
    user_id: String,
}

#[derive(Debug, Clone)]
pub(super) struct VoiceTurn {
    pub(super) id: Uuid,
    pub(super) token: CancellationToken,
    pub(super) interrupt_config: ChannelInterruptConfig,
    interrupt_state: Arc<StdMutex<VoiceTurnInterruptState>>,
    key: VoiceTurnKey,
}

#[derive(Debug)]
struct ActiveVoiceTurn {
    id: Uuid,
    token: CancellationToken,
    interrupt_state: Arc<StdMutex<VoiceTurnInterruptState>>,
}

#[derive(Debug, Default)]
pub(super) struct VoiceTurnRegistry {
    active: Mutex<HashMap<VoiceTurnKey, ActiveVoiceTurn>>,
}

#[derive(Debug, Default)]
struct VoiceTurnInterruptState {
    reason: Option<String>,
    stage: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VoiceTurnInterruption {
    pub(super) reason: String,
    pub(super) stage: &'static str,
}

impl VoiceTurnRegistry {
    pub(super) async fn begin(
        &self,
        channel_id: String,
        user_id: String,
        interrupt_config: ChannelInterruptConfig,
    ) -> VoiceTurn {
        let key = VoiceTurnKey {
            channel_id,
            user_id,
        };
        let turn = VoiceTurn {
            id: Uuid::new_v4(),
            key: key.clone(),
            token: CancellationToken::new(),
            interrupt_config,
            interrupt_state: Arc::new(StdMutex::new(VoiceTurnInterruptState::default())),
        };

        if turn.interrupt_config.enabled {
            let mut active = self.active.lock().await;
            if let Some(previous) = active.insert(
                key,
                ActiveVoiceTurn {
                    id: turn.id,
                    token: turn.token.clone(),
                    interrupt_state: turn.interrupt_state.clone(),
                },
            ) {
                set_interrupt_reason(
                    &previous.interrupt_state,
                    "superseded_by_new_voice_turn".to_string(),
                );
                previous.token.cancel();
            }
        }

        turn
    }

    pub(super) async fn interrupt(&self, channel_id: &str, user_id: &str, reason: &str) -> bool {
        let key = VoiceTurnKey {
            channel_id: channel_id.to_string(),
            user_id: user_id.to_string(),
        };
        let Some((token, interrupt_state)) = self
            .active
            .lock()
            .await
            .get(&key)
            .map(|turn| (turn.token.clone(), turn.interrupt_state.clone()))
        else {
            return false;
        };
        set_interrupt_reason(&interrupt_state, reason.to_string());
        token.cancel();
        true
    }

    pub(super) async fn finish(&self, turn: &VoiceTurn) {
        if !turn.interrupt_config.enabled {
            return;
        }
        let mut active = self.active.lock().await;
        if active
            .get(&turn.key)
            .map(|active| active.id == turn.id)
            .unwrap_or(false)
        {
            active.remove(&turn.key);
        }
    }
}

impl VoiceTurn {
    pub(super) fn interruption(&self) -> VoiceTurnInterruption {
        let state = self
            .interrupt_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        VoiceTurnInterruption {
            reason: state
                .reason
                .clone()
                .unwrap_or_else(|| "interrupted".to_string()),
            stage: state.stage.unwrap_or("unknown"),
        }
    }

    pub(super) fn mark_interrupted_stage(&self, stage: &'static str) {
        let mut state = self
            .interrupt_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.stage.is_none() {
            state.stage = Some(stage);
        }
    }
}

pub(super) fn turn_is_interrupted(turn: Option<&VoiceTurn>) -> bool {
    turn.is_some_and(|turn| turn.interrupt_config.enabled && turn.token.is_cancelled())
}

pub(super) async fn cancellable_stage<T, F>(
    turn: Option<&VoiceTurn>,
    stage: &'static str,
    fut: F,
) -> anyhow::Result<Option<T>>
where
    F: Future<Output = anyhow::Result<T>>,
{
    let Some(turn) = turn.filter(|turn| turn.interrupt_config.enabled) else {
        return fut.await.map(Some);
    };

    tokio::select! {
        _ = turn.token.cancelled() => {
            turn.mark_interrupted_stage(stage);
            tracing::info!(
                channel_id = %turn.key.channel_id,
                user_id = %turn.key.user_id,
                stage,
                "voice turn interrupted"
            );
            Ok(None)
        }
        result = fut => result.map(Some),
    }
}

fn set_interrupt_reason(state: &Arc<StdMutex<VoiceTurnInterruptState>>, reason: String) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.reason.is_none() {
        state.reason = Some(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interrupt_config() -> ChannelInterruptConfig {
        ChannelInterruptConfig {
            enabled: true,
            cancel_streaming_tts_on_interrupt: true,
            discard_interrupted_turn_history: true,
        }
    }

    #[tokio::test]
    async fn explicit_interrupt_records_reason_and_stage() {
        let registry = VoiceTurnRegistry::default();
        let turn = registry
            .begin(
                "embedded".to_string(),
                "embedded-user".to_string(),
                interrupt_config(),
            )
            .await;

        assert!(
            registry
                .interrupt("embedded", "embedded-user", "wakeword_during_thinking")
                .await
        );
        turn.mark_interrupted_stage("llm");

        assert_eq!(
            turn.interruption(),
            VoiceTurnInterruption {
                reason: "wakeword_during_thinking".to_string(),
                stage: "llm",
            }
        );
    }

    #[tokio::test]
    async fn beginning_new_turn_cancels_previous_with_superseded_reason() {
        let registry = VoiceTurnRegistry::default();
        let first = registry
            .begin(
                "embedded".to_string(),
                "embedded-user".to_string(),
                interrupt_config(),
            )
            .await;
        let _second = registry
            .begin(
                "embedded".to_string(),
                "embedded-user".to_string(),
                interrupt_config(),
            )
            .await;

        assert!(first.token.is_cancelled());
        assert_eq!(first.interruption().reason, "superseded_by_new_voice_turn");
    }
}
