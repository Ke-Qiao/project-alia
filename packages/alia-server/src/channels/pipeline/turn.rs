use std::collections::HashMap;
use std::future::Future;

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
    key: VoiceTurnKey,
}

#[derive(Debug)]
struct ActiveVoiceTurn {
    id: Uuid,
    token: CancellationToken,
}

#[derive(Debug, Default)]
pub(super) struct VoiceTurnRegistry {
    active: Mutex<HashMap<VoiceTurnKey, ActiveVoiceTurn>>,
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
        };

        if turn.interrupt_config.enabled {
            let mut active = self.active.lock().await;
            if let Some(previous) = active.insert(
                key,
                ActiveVoiceTurn {
                    id: turn.id,
                    token: turn.token.clone(),
                },
            ) {
                previous.token.cancel();
            }
        }

        turn
    }

    pub(super) async fn interrupt(&self, channel_id: &str, user_id: &str) -> bool {
        let key = VoiceTurnKey {
            channel_id: channel_id.to_string(),
            user_id: user_id.to_string(),
        };
        let Some(turn) = self
            .active
            .lock()
            .await
            .get(&key)
            .map(|turn| turn.token.clone())
        else {
            return false;
        };
        turn.cancel();
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
