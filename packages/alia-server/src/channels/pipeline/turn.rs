use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::channels::adapter::ChannelInterruptConfig;
use crate::modules::memory::context::{ContextBinding, ContextLease, GenerationContext};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct VoiceTurnKey {
    channel_id: String,
    user_id: String,
}

#[derive(Clone)]
pub(super) struct VoiceTurn {
    /// Response audit identity; live session ownership is in voice_scope.
    pub(super) id: Uuid,
    pub(super) response_id: Arc<std::sync::OnceLock<Uuid>>,
    pub(super) output_channel: Option<Arc<dyn crate::channels::adapter::ChannelAdapter>>,
    pub(super) output: Option<Arc<crate::modules::delivery::OutputProducer>>,
    pub(super) received_at: chrono::DateTime<chrono::Utc>,
    /// Required for production inbound sources; synthetic turns start unstamped.
    pub(super) source_privacy_epoch: Option<i64>,
    pub(super) source_relationship_epoch: Option<i64>,
    /// Committed user source shared by every tool and output task of this turn.
    pub(super) source_event_id: Arc<std::sync::OnceLock<Uuid>>,
    /// Actual model inputs may grow through tools until response preparation.
    /// Outputs and derived writes then share that frozen lease.
    pub(super) context: Arc<Mutex<TurnContext>>,
    pub(super) token: CancellationToken,
    pub(super) interrupt_config: ChannelInterruptConfig,
    pub(super) voice_scope: Option<VoiceSessionScope>,
    interrupt_state: Arc<StdMutex<VoiceTurnInterruptState>>,
    key: VoiceTurnKey,
}

#[derive(Debug, Default)]
pub(super) struct TurnContext {
    pub(super) affect_dirty: bool,
    pub(super) user_appraised: bool,
    pub(super) recall_digest: Option<String>,
    pub(super) generation: Option<GenerationContext>,
    pub(super) notice: Option<ContextLease>,
    pub(super) frozen: bool,
}

impl TurnContext {
    pub(super) fn lease(&self) -> anyhow::Result<&ContextLease> {
        self.generation
            .as_ref()
            .map(GenerationContext::lease)
            .or(self.notice.as_ref())
            .ok_or_else(|| anyhow::anyhow!("turn has no materialized context"))
    }
}

impl std::fmt::Debug for VoiceTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceTurn")
            .field("id", &self.id)
            .field("voice_scope", &self.voice_scope)
            .finish_non_exhaustive()
    }
}

pub(super) use alia_core::delivery::VoiceScope as VoiceSessionScope;

#[derive(Debug)]
struct ActiveVoiceTurn {
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
        token: CancellationToken,
    ) -> VoiceTurn {
        self.register(channel_id, user_id, Uuid::new_v4(), interrupt_config, token)
            .await
    }

    /// Register a response audit identity with the owner's cancellation token.
    pub(super) async fn register(
        &self,
        channel_id: String,
        user_id: String,
        id: Uuid,
        interrupt_config: ChannelInterruptConfig,
        token: CancellationToken,
    ) -> VoiceTurn {
        let key = VoiceTurnKey {
            channel_id,
            user_id,
        };
        let turn = VoiceTurn {
            id,
            response_id: Arc::default(),
            output_channel: None,
            output: None,
            received_at: chrono::Utc::now(),
            source_privacy_epoch: None,
            source_relationship_epoch: None,
            source_event_id: Arc::default(),
            context: Arc::default(),
            key: key.clone(),
            token,
            interrupt_config,
            voice_scope: None,
            interrupt_state: Arc::new(StdMutex::new(VoiceTurnInterruptState::default())),
        };

        if turn.interrupt_config.enabled {
            let mut active = self.active.lock().await;
            if turn.token.is_cancelled() {
                return turn;
            }
            if let Some(previous) = active.insert(
                key,
                ActiveVoiceTurn {
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
            .map(|active| Arc::ptr_eq(&active.interrupt_state, &turn.interrupt_state))
            .unwrap_or(false)
        {
            active.remove(&turn.key);
        }
    }
}

impl VoiceTurn {
    pub(super) async fn generation_context(&self) -> anyhow::Result<GenerationContext> {
        self.context
            .lock()
            .await
            .generation
            .clone()
            .ok_or_else(|| anyhow::anyhow!("turn has no generation context"))
    }

    pub(super) async fn context_binding(&self) -> anyhow::Result<ContextBinding> {
        Ok(self.context.lock().await.lease()?.binding())
    }

    pub(super) fn for_notice(&self, lease: ContextLease) -> Self {
        let mut notice = self.clone();
        notice.response_id = Arc::default();
        notice.context = Arc::new(Mutex::new(TurnContext {
            generation: None,
            notice: Some(lease),
            frozen: false,
            ..Default::default()
        }));
        notice
    }

    pub(super) fn request(token: CancellationToken) -> Self {
        Self {
            id: Uuid::new_v4(),
            response_id: Arc::default(),
            output_channel: None,
            output: None,
            received_at: chrono::Utc::now(),
            source_privacy_epoch: None,
            source_relationship_epoch: None,
            source_event_id: Arc::default(),
            context: Arc::default(),
            token,
            interrupt_config: ChannelInterruptConfig::default(),
            voice_scope: None,
            interrupt_state: Arc::default(),
            key: VoiceTurnKey {
                channel_id: String::new(),
                user_id: String::new(),
            },
        }
    }

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
    turn.is_some_and(|turn| turn.token.is_cancelled())
}

pub(super) async fn cancellable_stage<T, F>(
    turn: Option<&VoiceTurn>,
    stage: &'static str,
    fut: F,
) -> anyhow::Result<Option<T>>
where
    F: Future<Output = anyhow::Result<T>>,
{
    let Some(turn) = turn else {
        return fut.await.map(Some);
    };

    tokio::select! {
        biased;
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
        result = async {
            if let Some(output) = &turn.output { output.wait_open().await?; }
            // A hold never drops and recreates an in-flight request. Retain its
            // bounded result until the same original owner is allowed onward.
            let result = fut.await?;
            if let Some(output) = &turn.output { output.wait_open().await?; }
            Ok::<_, anyhow::Error>(result)
        } => result.map(Some),
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
                CancellationToken::new(),
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
                CancellationToken::new(),
            )
            .await;
        let _second = registry
            .begin(
                "embedded".to_string(),
                "embedded-user".to_string(),
                interrupt_config(),
                CancellationToken::new(),
            )
            .await;

        assert!(first.token.is_cancelled());
        assert_eq!(first.interruption().reason, "superseded_by_new_voice_turn");
    }
    #[tokio::test]
    async fn cancelled_stage_never_polls_even_a_ready_future() {
        let turn = VoiceTurn::request(CancellationToken::new());
        turn.token.cancel();
        let result = cancellable_stage(Some(&turn), "ready", async {
            panic!("cancelled work was polled");
            #[allow(unreachable_code)]
            Ok::<_, anyhow::Error>(())
        })
        .await
        .unwrap();
        assert!(result.is_none());
        assert!(turn_is_interrupted(Some(&turn))); // legacy interrupt policy may be disabled
    }

    #[tokio::test]
    async fn finishing_old_generation_does_not_unregister_current_session() {
        let registry = VoiceTurnRegistry::default();
        let session_id = Uuid::new_v4();
        let first_token = CancellationToken::new();
        let first = registry
            .register(
                "embedded".into(),
                "user".into(),
                session_id,
                interrupt_config(),
                first_token.clone(),
            )
            .await;
        let second_token = CancellationToken::new();
        let second = registry
            .register(
                "embedded".into(),
                "user".into(),
                session_id,
                interrupt_config(),
                second_token.clone(),
            )
            .await;
        assert_eq!(first.id, session_id);
        assert_eq!(second.id, session_id);
        assert!(first_token.is_cancelled());
        registry.finish(&first).await;
        assert!(registry.interrupt("embedded", "user", "barge_in").await);
        assert!(second_token.is_cancelled());
    }

    #[tokio::test]
    async fn late_cancelled_input_does_not_supersede_live_generation() {
        let registry = VoiceTurnRegistry::default();
        let token = CancellationToken::new();
        let live = registry
            .register(
                "embedded".into(),
                "user".into(),
                Uuid::new_v4(),
                interrupt_config(),
                token.clone(),
            )
            .await;
        let stale = CancellationToken::new();
        stale.cancel();
        let old = registry
            .register(
                "embedded".into(),
                "user".into(),
                Uuid::new_v4(),
                interrupt_config(),
                stale,
            )
            .await;
        registry.finish(&old).await;
        assert!(!token.is_cancelled());
        assert!(registry.interrupt("embedded", "user", "barge_in").await);
        assert!(live.token.is_cancelled());
    }
}
