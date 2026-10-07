//! Tests the ingress/response chain with controlled model and media collaborators.
//! Successful delivery paths use real PostgreSQL. Interaction cannot access STT.
use super::context::ensure_turn_context;
use super::error_notice::{InteractionErrorStage, notify_interaction_error};
use super::generation::{TextResult, process_text};
use super::persistence::{PendingTextPersistence, persist_text_interaction};
use super::policy::InteractionPolicies;
use super::runtime::{process_event, spawn_interaction};
use super::services::InteractionServices;
use super::streaming::tts_bytes_to_pcm_payload;
use super::turn::{VoiceSessionScope, VoiceTurn, VoiceTurnRegistry};
use super::voice_input::RecognizedVoiceInput;
use super::{delivery, error_notice, supervision, tools, voice};
use crate::channels::ChannelRegistry;
use crate::channels::adapter::AudioPayload;
use crate::channels::adapter::{ChannelAdapter, ChannelEvent, ChannelTarget};
use crate::event::EventBus;
use crate::modules::memory::context::ContextBinding;
use crate::modules::platform::lifecycle::ModuleLifecycle;
use crate::providers::adapter::{
    ChatMessage, ConversationEventEntry, LlmAdapter, RouteDecision, RouterInput,
};
use crate::providers::adapter::{ConversationBackend, RouterStrategy};
use crate::providers::sensor::SensorRegistry;
use alia_core::delivery::ObservationEnd;
use alia_core::text_delivery::{TextFrame, TextIdentity, TextProgress};
use async_trait::async_trait;
use futures::Stream;
use sha2::{Digest, Sha256};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
mod cache;
mod context;
mod memory_search;
mod text_delivery;

#[derive(Default)]
struct Conversation {
    messages: Mutex<Vec<ChatMessage>>,
    events: Mutex<Vec<ConversationEventEntry>>,
    source_times: Mutex<Vec<chrono::DateTime<chrono::Utc>>>,
    source_epochs: Mutex<Vec<i64>>,
    source_relationship_epochs: Mutex<Vec<i64>>,
    db: Option<sea_orm::DatabaseConnection>,
    fail_session: AtomicBool,
    fail_user: AtomicBool,
    fail_assistant: AtomicBool,
}
impl ModuleLifecycle for Conversation {
    fn name(&self) -> &'static str {
        "test_conversation"
    }
}
#[async_trait]
impl ConversationBackend for Conversation {
    async fn get_or_create_active_session(
        &self,
        channel: &str,
        user: &str,
    ) -> anyhow::Result<Uuid> {
        anyhow::ensure!(
            !self.fail_session.load(Ordering::SeqCst),
            "injected session failure"
        );
        if let Some(db) = &self.db {
            use sea_orm::ConnectionTrait;
            let session = if channel == "test" {
                Uuid::from_u128(100)
            } else {
                stable_user_uuid(&format!("{channel}:{user}"))
            };
            db.execute(sea_orm::Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,"INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",vec![session.into(),channel.into(),user.into()])).await?;
            return Ok(session);
        }
        Ok(Uuid::nil())
    }
    async fn create_session(&self, _: &str, _: &str) -> anyhow::Result<Uuid> {
        Ok(Uuid::nil())
    }
    async fn list_sessions(
        &self,
        _: &str,
        _: &str,
    ) -> anyhow::Result<Vec<(Uuid, Option<String>, chrono::DateTime<chrono::Utc>)>> {
        Ok(Vec::new())
    }
    async fn set_active_session(&self, _: &str, _: &str, _: Uuid) -> anyhow::Result<()> {
        Ok(())
    }
    async fn get_history(&self, _: Uuid, _: u64) -> anyhow::Result<Vec<ChatMessage>> {
        Ok(Vec::new())
    }
    async fn append_message(&self, _: Uuid, message: ChatMessage) -> anyhow::Result<()> {
        anyhow::ensure!(
            message.role != "assistant" || !self.fail_assistant.load(Ordering::SeqCst),
            "injected assistant persistence failure"
        );
        self.messages.lock().unwrap().push(message);
        Ok(())
    }
    async fn append_sourced_message(
        &self,
        owner: Uuid,
        session: Uuid,
        message: ChatMessage,
        received_at: chrono::DateTime<chrono::Utc>,
        source_privacy_epoch: i64,
        source_relationship_epoch: i64,
    ) -> anyhow::Result<Uuid> {
        anyhow::ensure!(
            !self.fail_user.load(Ordering::SeqCst),
            "injected source persistence failure"
        );
        anyhow::ensure!(message.role == "user", "source must be user input");
        let source = Uuid::new_v4();
        if let Some(db) = &self.db {
            use sea_orm::ConnectionTrait;
            db.execute(sea_orm::Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,"INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch,source_relationship_epoch) VALUES($1,$2,$3,'user',$4,$5,$6,$7)",vec![source.into(),session.into(),owner.to_string().into(),message.content.clone().into(),received_at.into(),source_privacy_epoch.into(),source_relationship_epoch.into()])).await?;
        }
        self.append_message(session, message).await?;
        self.source_times.lock().unwrap().push(received_at);
        self.source_epochs
            .lock()
            .unwrap()
            .push(source_privacy_epoch);
        self.source_relationship_epochs
            .lock()
            .unwrap()
            .push(source_relationship_epoch);
        Ok(source)
    }
    async fn commit_response(
        &self,
        owner: Uuid,
        session: Uuid,
        response: Uuid,
        message: ChatMessage,
    ) -> anyhow::Result<Uuid> {
        anyhow::ensure!(
            !self.fail_assistant.load(Ordering::SeqCst),
            "injected assistant persistence failure"
        );
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("test conversation has no durable delivery ledger"))?;
        use sea_orm::{ConnectionTrait, TransactionTrait};
        let tx = db.begin().await?;
        let result = crate::modules::delivery::repository::DeliveryRepository::new(db.clone())
            .mark_generated_in_transaction(
                &tx,
                owner,
                response,
                Uuid::new_v4(),
                serde_json::json!({"text":message.content}),
            )
            .await?;
        let id = result.assistant_message_id.unwrap();
        tx.execute(sea_orm::Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,"INSERT INTO chat_messages(id,session_id,role,content) VALUES($1,$2,'assistant',$3) ON CONFLICT DO NOTHING",vec![id.into(),session.into(),message.content.clone().into()])).await?;
        tx.commit().await?;
        self.append_message(session, message).await?;
        Ok(id)
    }
    async fn append_event(&self, _: Uuid, event: ConversationEventEntry) -> anyhow::Result<()> {
        let mut events = self.events.lock().unwrap();
        assert!(
            !events
                .iter()
                .any(|entry| entry.turn_id == event.turn_id && entry.sequence == event.sequence),
            "trace key collision"
        );
        events.push(event);
        Ok(())
    }
}

struct ObservedLlm {
    started: mpsc::UnboundedSender<String>,
    block: bool,
}
impl ModuleLifecycle for ObservedLlm {
    fn name(&self) -> &'static str {
        "test_llm"
    }
}
#[async_trait]
impl LlmAdapter for ObservedLlm {
    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String> {
        let text = messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .unwrap()
            .content
            .clone();
        self.started.send(text).unwrap();
        if self.block {
            std::future::pending::<()>().await;
        }
        Ok("recognized reply".into())
    }
    async fn chat_stream(
        &self,
        _: &[ChatMessage],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> {
        panic!("test channel is buffered");
    }
}
struct DirectRouter;
impl ModuleLifecycle for DirectRouter {
    fn name(&self) -> &'static str {
        "test_router"
    }
}
#[async_trait]
impl RouterStrategy for DirectRouter {
    async fn route(&self, _: &RouterInput) -> anyhow::Result<RouteDecision> {
        Ok(RouteDecision::MajorLlm {
            system_prompt: None,
        })
    }
}
#[derive(Default)]
struct OutputChannel {
    id: String,
    owner: Option<Uuid>,
    streaming: bool,
    sequence: std::sync::atomic::AtomicU64,
    first_gate: Option<Arc<tokio::sync::Semaphore>>,
    registration_entered: Option<Arc<tokio::sync::Semaphore>>,
    registration_release: Option<Arc<tokio::sync::Semaphore>>,
    backend: std::sync::OnceLock<alia_core::delivery::SubmissionBackend>,
    bindings: Mutex<
        std::collections::HashMap<VoiceSessionScope, alia_core::output_control::OutputBinding>,
    >,
    controls: Mutex<
        std::collections::HashMap<
            VoiceSessionScope,
            alia_core::playback_control::PlaybackControlState,
        >,
    >,
    control_sequence: std::sync::atomic::AtomicU64,
    texts: Mutex<Vec<String>>,
    text_instance: std::sync::OnceLock<Uuid>,
    text_progress: Mutex<std::collections::HashMap<Uuid, TextProgress>>,
    audio: Mutex<Vec<AudioPayload>>,
}
impl OutputChannel {
    fn finish_text(
        &self,
        identity: TextIdentity,
        end: ObservationEnd,
    ) -> anyhow::Result<TextProgress> {
        let mut writers = self.text_progress.lock().unwrap();
        let progress = writers
            .get_mut(&identity.text_id)
            .ok_or_else(|| anyhow::anyhow!("unknown text writer"))?;
        anyhow::ensure!(progress.identity == identity, "text owner changed");
        if progress.observation_end.is_none() {
            progress.receipt_sequence += 1;
            progress.observation_end = Some(end);
        }
        Ok(progress.clone())
    }

    fn backend(&self) -> alia_core::delivery::SubmissionBackend {
        self.backend
            .get_or_init(|| alia_core::delivery::SubmissionBackend {
                instance_id: Uuid::new_v4(),
                boundary: alia_core::delivery::SubmissionBoundary::CpalCallbackBuffer,
                output_sample_rate_hz: 24000,
                output_channels: 1,
            })
            .clone()
    }
}
impl ModuleLifecycle for OutputChannel {
    fn name(&self) -> &'static str {
        "test_output"
    }
}
#[async_trait]
impl ChannelAdapter for OutputChannel {
    fn channel_type(&self) -> &'static str {
        "test"
    }
    fn channel_id(&self) -> &str {
        if self.id.is_empty() { "test" } else { &self.id }
    }
    fn owner_user_id(&self) -> Option<Uuid> {
        self.owner
    }
    async fn register_output(
        &self,
        binding: alia_core::output_control::OutputBinding,
    ) -> anyhow::Result<alia_core::output_control::OutputRegistration> {
        let original = *self
            .bindings
            .lock()
            .unwrap()
            .entry(binding.scope)
            .or_insert(binding);
        assert_eq!(original, binding);
        self.controls
            .lock()
            .unwrap()
            .entry(binding.scope)
            .or_insert_with(|| {
                alia_core::playback_control::PlaybackControlState::new(binding.scope).unwrap()
            });
        if let Some(entered) = &self.registration_entered {
            entered.add_permits(1);
        }
        if let Some(release) = &self.registration_release {
            release.acquire().await?.forget();
        }
        Ok(alia_core::output_control::OutputRegistration {
            binding,
            backend: self.backend(),
            maximum_hold_ms: 120_000,
            initiative_capture_gate_version: 0,
        })
    }
    async fn retire_voice_output(
        &self,
        request: &alia_core::output_control::OutputRetirementRequest,
    ) -> anyhow::Result<alia_core::output_control::OutputRetirementReceipt> {
        use alia_core::output_control::{OutputRetirementEvidence, OutputRetirementReceipt};
        request.validate()?;
        let binding = self.bindings.lock().unwrap().get(&request.scope).copied();
        if let Some(binding) = binding {
            assert_eq!(request.binding, Some(binding));
        }
        if let Some(expected) = &request.expected_backend {
            assert_eq!(expected, &self.backend());
        }
        if let Some(control) = self.controls.lock().unwrap().get_mut(&request.scope) {
            control.retire(alia_core::playback_control::RetiredReason::Closed);
        }
        // This software-only fixture has no native writes beyond its explicit
        // scope gate. Media observations remain the deliver_audio facts.
        let evidence = if binding.is_some() {
            OutputRetirementEvidence::Settled {
                backend: self.backend(),
                progress: vec![],
            }
        } else {
            OutputRetirementEvidence::NoNativeOwner
        };
        let receipt = OutputRetirementReceipt {
            scope: request.scope,
            binding: request.binding,
            evidence,
        };
        receipt.validate(request)?;
        Ok(receipt)
    }
    async fn control_output(
        &self,
        command: &alia_core::output_control::BoundControlCommand,
    ) -> anyhow::Result<alia_core::output_control::BoundControlReceipt> {
        let mut controls = self.controls.lock().unwrap();
        let control = controls.get_mut(&command.binding.scope).unwrap();
        Ok(alia_core::output_control::BoundControlReceipt {
            observation_sequence: self.control_sequence.fetch_add(1, Ordering::SeqCst) + 1,
            command: command.clone(),
            intent_digest: command.intent_digest(),
            receipt: control.apply(&command.command, 0, true),
            backend: self.backend(),
            mode: alia_core::output_control::ControlMode::SoftwareGate,
            native_writes_quiescent: true,
            progress: vec![],
        })
    }
    fn reserve_playback(
        &self,
        target: &ChannelTarget,
        playback_id: Uuid,
    ) -> anyhow::Result<alia_core::delivery::PlaybackIdentity> {
        Ok(alia_core::delivery::PlaybackIdentity {
            scope: target
                .voice
                .ok_or_else(|| anyhow::anyhow!("playback requires an original voice scope"))?,
            playback_id,
            playback_sequence: self.sequence.fetch_add(1, Ordering::SeqCst) + 1,
        })
    }
    async fn deliver_audio(
        &self,
        _: &ChannelTarget,
        identity: alia_core::delivery::PlaybackIdentity,
        mut audio: crate::channels::adapter::AudioPayloadStream,
        observer: Arc<dyn crate::channels::adapter::PlaybackObserver>,
        _: CancellationToken,
    ) -> anyhow::Result<crate::channels::adapter::AudioDeliveryReport> {
        use futures::StreamExt;
        observer.start(24000, 1).await?;
        let mut frames = 0u64;
        while let Some(payload) = audio.next().await {
            let payload = payload?;
            let pcm = tts_bytes_to_pcm_payload(payload.data.clone())?;
            frames += pcm.data.len() as u64 / (u64::from(pcm.channels) * 2);
            observer.offered(frames).await?;
            self.audio.lock().unwrap().push(payload);
        }
        if identity.playback_sequence == 1 {
            if let Some(gate) = &self.first_gate {
                gate.acquire().await.unwrap().forget();
            }
        }
        let progress = alia_core::delivery::PlaybackProgress {
            identity,
            receipt_sequence: 1,
            accepted_source_frames: frames,
            submitted_source_frames: Some(frames),
            presented_source_frames: None,
            end_accepted: true,
            backend: Some(self.backend()),
            observation_end: Some(alia_core::delivery::ObservationEnd::SubmissionFinished),
        };
        observer.progress(progress.clone()).await?;
        Ok(crate::channels::adapter::AudioDeliveryReport {
            progress: Some(progress),
            observation_end: alia_core::delivery::ObservationEnd::SubmissionFinished,
        })
    }
    fn capabilities(&self) -> crate::channels::adapter::ChannelCapabilities {
        crate::channels::adapter::ChannelCapabilities {
            text: true,
            streaming: self.streaming,
            audio_streaming: self.streaming,
            voice_output: true,
            ..Default::default()
        }
    }
    fn reserve_text(&self, _: &ChannelTarget, text_id: Uuid) -> anyhow::Result<TextIdentity> {
        let identity = TextIdentity {
            text_id,
            channel_instance_id: *self.text_instance.get_or_init(Uuid::new_v4),
        };
        anyhow::ensure!(identity.is_valid(), "invalid text identity");
        let mut writers = self.text_progress.lock().unwrap();
        anyhow::ensure!(!writers.contains_key(&text_id), "text identity reused");
        writers.insert(
            text_id,
            TextProgress {
                identity,
                receipt_sequence: 0,
                accepted_utf8_bytes: 0,
                written_utf8_bytes: None,
                write_boundary: None,
                end_accepted: false,
                observation_end: None,
            },
        );
        Ok(identity)
    }
    async fn deliver_text(
        &self,
        _: &ChannelTarget,
        identity: TextIdentity,
        mut text: crate::channels::adapter::TextPayloadStream,
        observer: Arc<dyn crate::channels::adapter::TextObserver>,
        cancel: CancellationToken,
    ) -> anyhow::Result<crate::channels::adapter::TextDeliveryReport> {
        use futures::StreamExt;
        observer.start().await?;
        let mut buffered = String::new();
        let mut sequence = 0;
        loop {
            let frame = tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                frame = text.next() => frame,
            };
            let Some(frame) = frame else { break };
            match frame? {
                TextFrame::Chunk(chunk) => {
                    anyhow::ensure!(
                        chunk.sequence == sequence && chunk.utf8_offset == buffered.len() as u64,
                        "text sequence changed"
                    );
                    observer.offered(&chunk).await?;
                    let mut writers = self.text_progress.lock().unwrap();
                    let progress = writers.get_mut(&identity.text_id).unwrap();
                    anyhow::ensure!(
                        progress.identity == identity && progress.observation_end.is_none(),
                        "text writer retired"
                    );
                    buffered.push_str(&chunk.text);
                    sequence += 1;
                    progress.accepted_utf8_bytes = buffered.len() as u64;
                }
                TextFrame::End { chunks, utf8_bytes } => {
                    anyhow::ensure!(
                        chunks == sequence && utf8_bytes == buffered.len() as u64,
                        "text End changed"
                    );
                    observer.end(chunks, utf8_bytes).await?;
                    let mut writers = self.text_progress.lock().unwrap();
                    let progress = writers.get_mut(&identity.text_id).unwrap();
                    if progress.identity == identity
                        && progress.observation_end.is_none()
                        && !cancel.is_cancelled()
                    {
                        progress.end_accepted = true;
                        self.texts.lock().unwrap().push(buffered);
                    }
                    break;
                }
            }
        }
        let progress = self.finish_text(
            identity,
            if cancel.is_cancelled() {
                ObservationEnd::Cancelled {
                    reason: "original text writer cancelled".into(),
                }
            } else {
                // This fixture records memory acceptance only; it does not write
                // stdout and cannot manufacture a native write checkpoint.
                ObservationEnd::Unknown {
                    reason: "in-memory fixture has no native text write evidence".into(),
                }
            },
        )?;
        observer.progress(progress.clone()).await?;
        Ok(crate::channels::adapter::TextDeliveryReport {
            observation_end: progress.observation_end.clone().unwrap(),
            progress: Some(progress),
        })
    }
    async fn cancel_text(
        &self,
        identity: TextIdentity,
        reason: &str,
    ) -> anyhow::Result<TextProgress> {
        self.finish_text(
            identity,
            ObservationEnd::Cancelled {
                reason: reason.into(),
            },
        )
    }
}
pub(super) struct Fixture {
    pub(super) modules: Arc<InteractionServices>,
    channels: Arc<ChannelRegistry>,
    sensors: Arc<SensorRegistry>,
    output: Arc<OutputChannel>,
    conversation: Arc<Conversation>,
    policy: InteractionPolicies,
    started: mpsc::UnboundedReceiver<String>,
}
impl Fixture {
    pub(super) async fn new(block: bool) -> Self {
        let config: crate::config::ServerConfig =
            toml::from_str(include_str!("../../../../../config/server.example.toml")).unwrap();
        let (started_tx, started) = mpsc::unbounded_channel();
        let llm = Arc::new(ObservedLlm {
            started: started_tx,
            block,
        });
        let conversation = Arc::new(Conversation::default());
        let channels = Arc::new(ChannelRegistry::new());
        let sensors = Arc::new(SensorRegistry::new(Default::default()));
        let output = Arc::new(OutputChannel::default());
        channels.register(output.clone()).await.unwrap();
        let modules = Arc::new(InteractionServices {
            context: Arc::new(crate::modules::memory::context::ContextService::new(
                sea_orm::DatabaseConnection::Disconnected,
                Arc::new(crate::providers::embedding::NoopEmbedding),
            )),
            delivery: crate::modules::delivery::DeliveryService::new(
                crate::modules::delivery::repository::DeliveryRepository::new(
                    sea_orm::DatabaseConnection::Disconnected,
                ),
                &CancellationToken::new(),
            ),
            llm: llm.clone(),
            minor_llm: llm.clone(),
            major_llm: llm,
            memory_capture_llm: None,
            sentence_expression_llm: None,
            tts: Arc::new(crate::providers::tts::NoopTts),
            embedding: Arc::new(crate::providers::embedding::NoopEmbedding),
            conversation: conversation.clone(),
            memory: Arc::new(crate::modules::memory::MemoryService::new()),
            cache: Arc::new(crate::providers::cache::DbCache::new()),
            skills: Arc::new(crate::providers::skills::BuiltinSkills::new()),
            persona: Arc::new(crate::providers::persona::TemplatePersona::new()),
            router: Arc::new(DirectRouter),
            tools: Arc::new(crate::tools::BuiltinToolRegistry::new()),
            expression: Arc::new(crate::expression::ExpressionRuntime::from_config(&config)),
        });
        let mut policy = InteractionPolicies::new(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
        );
        policy.cache.enabled = false;
        policy.memory.enabled = false;
        Self {
            modules,
            channels,
            sensors,
            output,
            conversation,
            policy,
            started,
        }
    }
}
fn input(text: &str) -> RecognizedVoiceInput {
    RecognizedVoiceInput {
        request_id: Uuid::new_v4(),
        output: None,
        source_received_at: chrono::Utc::now() - chrono::Duration::hours(1),
        source_privacy_epoch: 41,
        source_relationship_epoch: 0,
        channel_id: "test".into(),
        user_id: text.into(),
        transcript: text.into(),
        session_id: Uuid::new_v4(),
        device_boot_id: Uuid::new_v4(),
        generation_epoch: 7,
        cancel: CancellationToken::new(),
    }
}

fn attach_output(
    modules: &InteractionServices,
    channel: Arc<dyn ChannelAdapter>,
    input: &mut RecognizedVoiceInput,
) {
    input.output = Some(
        modules
            .delivery
            .begin_voice_output(
                input.user_id.parse().unwrap(),
                input.request_id,
                VoiceSessionScope {
                    session_id: input.session_id,
                    device_boot_id: input.device_boot_id,
                    generation_epoch: input.generation_epoch,
                },
                channel,
                input.cancel.clone(),
                None,
            )
            .unwrap(),
    );
}

#[tokio::test]
async fn prepared_voice_error_before_register_ack_keeps_original_context_and_response() {
    struct UnexpectedTts;
    impl ModuleLifecycle for UnexpectedTts {
        fn name(&self) -> &'static str {
            "unexpected-error-tts"
        }
    }
    #[async_trait]
    impl crate::providers::adapter::TtsAdapter for UnexpectedTts {
        async fn synthesize_request(
            &self,
            _: &crate::providers::adapter::TtsRequest,
            _: crate::providers::tts::audio_budget::ResponseAudioBudget,
        ) -> anyhow::Result<crate::providers::tts::audio_budget::EncodedAudio> {
            panic!("prepared voice error started a replacement TTS request");
        }
        async fn synthesize_stream_request(
            &self,
            _: &crate::providers::adapter::TtsRequest,
            _: crate::providers::tts::audio_budget::ResponseAudioBudget,
        ) -> anyhow::Result<crate::providers::tts::audio_budget::TtsAudioStream> {
            panic!("prepared voice error started a replacement TTS stream");
        }
    }
    with_linked_fixture(|mut fixture| async move {
        use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
        Arc::get_mut(&mut fixture.modules).unwrap().tts = Arc::new(UnexpectedTts);
        let entered = Arc::new(tokio::sync::Semaphore::new(0));
        let channel = Arc::new(OutputChannel {
            owner: Some(Uuid::from_u128(42)),
            registration_entered: Some(entered.clone()),
            registration_release: Some(Arc::new(tokio::sync::Semaphore::new(0))),
            ..Default::default()
        });
        let user = Uuid::from_u128(42).to_string();
        let mut turn = VoiceTurn::request(CancellationToken::new());
        turn.output_channel = Some(channel.clone());
        turn.source_privacy_epoch = Some(41);
        turn.source_relationship_epoch = Some(0);
        let scope = VoiceSessionScope {
            session_id: Uuid::new_v4(),
            device_boot_id: Uuid::new_v4(),
            generation_epoch: 1,
        };
        turn.voice_scope = Some(scope);
        turn.output = Some(fixture.modules.delivery.begin_voice_output(
            Uuid::from_u128(42), turn.id, scope, channel.clone(), turn.token.clone(), None,
        ).unwrap());
        ensure_turn_context(&fixture.modules, &turn, "test", &user, "Read this answer")
            .await.unwrap();
        let mut prepare = Box::pin(delivery::prepare_response(
            &fixture.modules, Some(&turn), "test", &user, None, "assistant_response",
        ));
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::select! {
                result = prepare.as_mut() => panic!("prepare finished before Register barrier: {result:?}"),
                permit = entered.acquire() => permit.unwrap().forget(),
            }
        }).await.unwrap();
        let response_id = *turn.response_id.get().expect("prepare hid canonical ID behind Register ACK");
        assert!(turn.output.as_ref().unwrap().output().response_id().is_none());
        assert!(!turn.token.is_cancelled());
        let original = fixture.modules.delivery.get_response(Uuid::from_u128(42), response_id).await.unwrap();
        let db = fixture.conversation.db.as_ref().unwrap();
        let context_count = || async {
            db.query_one(Statement::from_string(DatabaseBackend::Postgres,
                "SELECT count(*) AS n FROM context_leases".to_owned()))
                .await.unwrap().unwrap().try_get::<i64>("", "n").unwrap()
        };
        let contexts_before = context_count().await;
        // Exercise the actual error handler before the output actor's own
        // cancellation becomes visible. An ACK failure and error notification
        // may race with that cancellation on a multithreaded executor.
        let error = anyhow::anyhow!("injected registration acknowledgement failure");
        let target = ChannelTarget { user_id: user.clone(), voice: Some(scope) };
        let (prepared, notified) = tokio::join!(prepare.as_mut(), notify_interaction_error(
            &fixture.modules, channel.as_ref(), &target, "test", &user,
            InteractionErrorStage::Delivery, Some("Read this answer"), Some(&error),
            false, true, Some(&turn),
        ));
        assert!(prepared.is_err());
        assert_eq!(context_count().await, contexts_before, "prepared error materialized a replacement Context");
        let responses = db.query_one(Statement::from_string(DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM delivery_responses".to_owned()))
            .await.unwrap().unwrap().try_get::<i64>("", "n").unwrap();
        assert_eq!(responses, 1, "prepared error created a replacement Response");
        let cancelled = fixture.modules.delivery.get_response(Uuid::from_u128(42), response_id).await.unwrap();
        assert!(cancelled.cancel_requested);
        assert_eq!(cancelled.context_id, original.context_id);
        assert_eq!(cancelled.key.request_id, turn.id);
        assert_eq!(notified.delivery, error_notice::InteractionErrorDelivery::Failed);
        assert!(channel.texts.lock().unwrap().is_empty());
        assert!(channel.audio.lock().unwrap().is_empty());
    }).await;
}

#[tokio::test]
async fn incomplete_history_does_not_publish_completion_or_memory_capture() {
    for fail_user in [true, false] {
        let fixture = Fixture::new(false).await;
        fixture
            .conversation
            .fail_user
            .store(fail_user, Ordering::SeqCst);
        fixture
            .conversation
            .fail_assistant
            .store(!fail_user, Ordering::SeqCst);
        let user = Uuid::new_v4();
        let bus = EventBus::new(8);
        let mut events = bus.subscribe();
        let pending = PendingTextPersistence {
            context: ContextBinding {
                context_id: Uuid::new_v4(),
                owner_id: user,
                request_id: Uuid::new_v4(),
                source_event_id: None,
                source_privacy_epoch: 0,
            },
            source_received_at: chrono::Utc::now(),
            source_privacy_epoch: 0,
            source_event_id: None,
            response_id: uuid::Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            user_id: user,
            user_id_text: user.to_string(),
            query_text: "This input must have durable provenance".into(),
            response_text: "A reply whose history write might fail".into(),
            channel_id: "test".into(),
            cache_write: None,
        };
        persist_text_interaction(
            &fixture.modules,
            pending,
            &fixture.policy.memory,
            &bus,
            None,
        )
        .await;
        assert!(
            events.try_recv().is_err(),
            "incomplete interaction published completion"
        );
        let messages = fixture.conversation.messages.lock().unwrap();
        assert!(
            messages.is_empty(),
            "missing provenance must not be manufactured after generation"
        );
        assert!(messages.iter().all(|message| message.role == "user"));
        assert!(
            fixture.conversation.events.lock().unwrap().is_empty(),
            "incomplete interaction reached memory capture audit"
        );
    }
}

#[tokio::test]
async fn failed_user_source_prevents_model_and_tool_processing() {
    let mut fixture = Fixture::new(false).await;
    fixture.conversation.fail_user.store(true, Ordering::SeqCst);
    let user = Uuid::new_v4().to_string();
    let mut turn = VoiceTurn::request(CancellationToken::new());
    turn.source_privacy_epoch = Some(0);
    turn.source_relationship_epoch = Some(0);
    let result = process_text(
        "Remember this new preference",
        &user,
        &fixture.modules,
        fixture.output.as_ref(),
        &ChannelTarget {
            user_id: user.clone(),
            voice: None,
        },
        "test",
        &EventBus::new(8),
        false,
        false,
        false,
        &fixture.policy.conversation,
        &fixture.policy.tools,
        &fixture.policy.cache,
        &fixture.policy.cache_policy,
        &fixture.policy.memory,
        Some(&turn),
        true,
    )
    .await;
    assert!(result.is_err());
    assert!(turn.source_event_id.get().is_none());
    assert!(
        fixture.started.try_recv().is_err(),
        "model ran despite failed user source write"
    );
    assert!(fixture.conversation.messages.lock().unwrap().is_empty());
    assert!(fixture.output.texts.lock().unwrap().is_empty());
}
#[tokio::test]
async fn recognized_speech_reaches_llm_and_shared_text_tts_without_stt() {
    with_linked_fixture(|mut fixture| async move {
        let mut input = input("Keep this exact recognized transcript.");
        input.user_id = Uuid::from_u128(42).to_string();
        attach_output(&fixture.modules, fixture.output.clone(), &mut input);
        let session_id = input.session_id;
        let source_received_at = input.source_received_at;
        voice::process_recognized_voice(
            input,
            &fixture.modules,
            &fixture.channels,
            &EventBus::new(8),
            &VoiceTurnRegistry::default(),
            false,
            &fixture.policy,
        )
        .await
        .unwrap();
        assert_eq!(
            fixture.started.try_recv().unwrap(),
            "Keep this exact recognized transcript."
        );
        assert_eq!(
            fixture.output.texts.lock().unwrap().as_slice(),
            &["recognized reply"]
        );
        assert_eq!(fixture.output.audio.lock().unwrap().len(), 1);
        assert_eq!(
            fixture.conversation.source_times.lock().unwrap().as_slice(),
            &[source_received_at],
            "processing a queued input must preserve the original audit time"
        );
        assert_eq!(
            fixture
                .conversation
                .source_epochs
                .lock()
                .unwrap()
                .as_slice(),
            &[41],
            "queued speech must keep its admitted source epoch"
        );
        assert_eq!(
            fixture
                .conversation
                .source_relationship_epochs
                .lock()
                .unwrap()
                .as_slice(),
            &[0],
            "queued speech must preserve its relationship admission through generation"
        );
        let messages = fixture.conversation.messages.lock().unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|m| (m.role.as_str(), m.content.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("user", "Keep this exact recognized transcript."),
                ("assistant", "recognized reply")
            ]
        );
        let events = fixture.conversation.events.lock().unwrap();
        let started = events
            .iter()
            .find(|event| event.event_type == "turn_started")
            .unwrap();
        assert_eq!(
            started.payload["voice_scope"]["session_id"],
            session_id.to_string()
        );
        assert_eq!(started.payload["voice_scope"]["generation_epoch"], 7);
        assert_ne!(
            started.turn_id, session_id,
            "response audit must not reuse a long-lived session identity"
        );
    })
    .await;
}

#[tokio::test]
async fn saturated_voice_queue_rejects_overflow_and_skips_cancelled_input() {
    with_linked_fixture(|mut fixture| async move {
        let (started_tx, mut started) = mpsc::unbounded_channel();
        let llm = Arc::new(ObservedLlm {
            started: started_tx,
            block: true,
        });
        let modules = Arc::get_mut(&mut fixture.modules).unwrap();
        modules.llm = llm.clone();
        modules.minor_llm = llm.clone();
        modules.major_llm = llm;
        let channels = Arc::new(ChannelRegistry::new());
        for index in 0..=supervision::MAX_CONCURRENT_REQUESTS {
            channels
                .register(Arc::new(OutputChannel {
                    id: format!("test-{index}"),
                    owner: Some(Uuid::from_u128(42)),
                    ..Default::default()
                }))
                .await
                .unwrap();
        }
        let queued_channel = format!("test-{}", supervision::MAX_CONCURRENT_REQUESTS);
        let request = |text: &str, channel_id: String| {
            let mut request = input(text);
            request.user_id = Uuid::from_u128(42).to_string();
            request.channel_id = channel_id;
            request
        };
        let (ingress, receiver) = crate::modules::interaction::voice_input::channel();
        let shutdown = CancellationToken::new();
        let pipeline = spawn_interaction(
            &EventBus::new(8),
            fixture.modules,
            channels,
            fixture.sensors,
            shutdown.clone(),
            false,
            Arc::new(RwLock::new(fixture.policy)),
            receiver,
        );
        let mut active_tokens = Vec::new();
        for index in 0..supervision::MAX_CONCURRENT_REQUESTS {
            let input = request(&format!("running-{index}"), format!("test-{index}"));
            active_tokens.push(input.cancel.clone());
            ingress.try_submit(input).unwrap();
        }
        for _ in 0..supervision::MAX_CONCURRENT_REQUESTS {
            tokio::time::timeout(std::time::Duration::from_secs(3), started.recv())
                .await
                .unwrap()
                .unwrap();
        }
        let stale = request("cancelled-queued", queued_channel.clone());
        stale.cancel.cancel();
        ingress.try_submit(stale).unwrap();
        for index in 0..7 {
            ingress
                .try_submit(request(&format!("queued-{index}"), queued_channel.clone()))
                .unwrap();
        }
        assert!(matches!(
            ingress.try_submit(request("overflow", queued_channel.clone())),
            Err(mpsc::error::TrySendError::Full(_))
        ));
        active_tokens[0].cancel();
        let next = tokio::time::timeout(std::time::Duration::from_secs(3), started.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next, "queued-0");
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(3), pipeline)
            .await
            .unwrap()
            .unwrap();
        assert!(
            started.try_recv().is_err(),
            "queued work started after shutdown"
        );
        assert!(fixture.output.texts.lock().unwrap().is_empty());
        assert!(fixture.output.audio.lock().unwrap().is_empty());
        assert!(matches!(
            ingress.try_submit(request("closed", queued_channel)),
            Err(mpsc::error::TrySendError::Closed(_))
        ));
    })
    .await;
}

static PIPELINE_DATABASE_CASES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

async fn with_linked_fixture<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    use futures::FutureExt;
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let _slot = PIPELINE_DATABASE_CASES.acquire().await.unwrap();
    let url = std::env::var("ALIA_TEST_DATABASE_URL")
        .expect("pipeline Delivery test requires isolated PostgreSQL");
    let admin = sea_orm::Database::connect(url.clone()).await.unwrap();
    let schema = format!("pipeline_delivery_{}", Uuid::new_v4().simple());
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
        .await
        .unwrap();
    let mut options = sea_orm::ConnectOptions::new(url);
    options
        .max_connections(4)
        .set_schema_search_path(format!("{schema},public"))
        .sqlx_logging(false);
    let db = sea_orm::Database::connect(options).await.unwrap();
    let delivery = crate::modules::delivery::DeliveryService::new(
        crate::modules::delivery::repository::DeliveryRepository::new(db.clone()),
        &CancellationToken::new(),
    );
    let appraisal_writer = std::sync::OnceLock::new();
    let result = std::panic::AssertUnwindSafe(Box::pin(async {
    crate::modules::platform::migrations::run(&db)
        .await
        .unwrap();
    let owner = Uuid::from_u128(42);
    db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,'pipeline@delivery.test','Pipeline test','not-a-login','user','active')",[owner.into()])).await.unwrap();
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO memory_privacy_state(user_id,privacy_epoch) VALUES($1,41)",
        [owner.into()],
    ))
    .await
    .unwrap();
    // Inputs below carry an already-admitted stamp, so perform the same
    // durable dual-domain admission before concurrent Context snapshots start.
    let admitted=crate::modules::memory::source::capture_source_epochs(&db,owner).await.unwrap();
    assert_eq!(admitted.privacy_epoch,41);
    assert_eq!(admitted.relationship_epoch,0);
    let mut fixture = Fixture::new(false).await;
    fixture.conversation = Arc::new(Conversation {
        db: Some(db.clone()),
        ..Default::default()
    });
    fixture.output = Arc::new(OutputChannel {
        owner: Some(owner),
        ..Default::default()
    });
    fixture.channels = Arc::new(ChannelRegistry::new());
    fixture
        .channels
        .register(fixture.output.clone())
        .await
        .unwrap();
    delivery.start().await.unwrap();
    let modules = Arc::get_mut(&mut fixture.modules).unwrap();
    modules.delivery = delivery.clone();
    modules.conversation = fixture.conversation.clone();
    modules.expression.attach_db(db.clone()).await.unwrap();
    appraisal_writer.set(modules.expression.clone()).unwrap();
    modules.context = Arc::new(crate::modules::memory::context::ContextService::new(
        db.clone(),
        modules.embedding.clone(),
    ));
    modules
        .context
        .initialize_policy(
            crate::modules::memory::context::ContextPolicy::from_config(
                &fixture.policy.memory,
                &fixture.policy.conversation,
            )
            .unwrap()
            .with_cache(&fixture.policy.cache)
            .unwrap(),
        )
        .await
        .unwrap();
        // Initializing the real tool host hashes this debug executable once.
        // Complete that fixture setup before timing pipeline behavior; on
        // Linux ARM the artifact read alone can consume the 15-second budget.
        let identity_owner = crate::modules::execution::ExecutionService::new(
            crate::modules::execution::repository::ExecutionRepository::new(db.clone()),
            Arc::new(crate::tools::BuiltinToolRegistry::new()),
            &fixture.policy.tools,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        drop(identity_owner);
        tokio::time::timeout(std::time::Duration::from_secs(15), Box::pin(test(fixture))).await
    }))
    .catch_unwind()
    .await;
    if let Some(writer) = appraisal_writer.get() {
        writer.shutdown().await.unwrap();
    }
    delivery.shutdown().await.unwrap();
    db.close().await.unwrap();
    admin
        .execute_unprepared(&format!("DROP SCHEMA {schema} CASCADE"))
        .await
        .unwrap();
    admin.close().await.unwrap();
    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("pipeline Delivery test exceeded15s"),
        Err(error) => std::panic::resume_unwind(error),
    }
}

#[tokio::test]
async fn sentence_delivery_presynthesizes_bounded_fragments_under_one_response() {
    run_sentence_budget_case(false, false).await;
}

#[tokio::test]
async fn sentence_tts_failure_is_not_committed_as_success_or_replayed() {
    run_sentence_budget_case(true, false).await;
}

#[tokio::test]
async fn sentence_hold_retains_in_flight_result_and_defers_next_tts() {
    run_sentence_budget_case(false, true).await;
}

async fn run_sentence_budget_case(fail_tts: bool, pause_first_request: bool) {
    struct Streaming;
    impl ModuleLifecycle for Streaming {
        fn name(&self) -> &'static str {
            "streaming-test"
        }
    }
    #[async_trait]
    impl LlmAdapter for Streaming {
        async fn chat(&self, _: &[ChatMessage]) -> anyhow::Result<String> {
            panic!("this case must use streaming")
        }
        async fn chat_stream(
            &self,
            _: &[ChatMessage],
        ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> {
            Ok(Box::pin(futures::stream::iter([
                Ok("First sentence. ".into()),
                Ok("Second sentence.".into()),
            ])))
        }
    }
    struct Speech {
        calls: std::sync::atomic::AtomicUsize,
        db: sea_orm::DatabaseConnection,
        owners: Mutex<Vec<Uuid>>,
        texts: Mutex<Vec<String>>,
        fail: bool,
        pause_first_request: bool,
        first_started: tokio::sync::Semaphore,
        first_release: tokio::sync::Semaphore,
        first_completed: tokio::sync::Semaphore,
        second_started: tokio::sync::Semaphore,
    }
    impl ModuleLifecycle for Speech {
        fn name(&self) -> &'static str {
            "speech-test"
        }
    }
    #[async_trait]
    impl crate::providers::adapter::TtsAdapter for Speech {
        async fn synthesize_request(
            &self,
            request: &crate::providers::adapter::TtsRequest,
            budget: crate::providers::tts::audio_budget::ResponseAudioBudget,
        ) -> anyhow::Result<crate::providers::tts::audio_budget::EncodedAudio> {
            use sea_orm::ConnectionTrait;
            let crate::providers::tts::audio_budget::AudioBudgetOwner::Response(id) =
                budget.owner()
            else {
                panic!("sentence TTS must use its real Response budget");
            };
            let row = self.db.query_one(sea_orm::Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT context_id, source_event_id FROM delivery_responses WHERE response_id=$1",
                [id.into()],
            )).await?.expect("Response must be committed before starting TTS");
            assert!(row.try_get::<Option<Uuid>>("", "context_id")?.is_some());
            assert!(
                row.try_get::<Option<Uuid>>("", "source_event_id")?
                    .is_some()
            );
            self.owners.lock().unwrap().push(id);
            self.texts.lock().unwrap().push(request.text.clone());
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                self.first_started.add_permits(1);
                if self.pause_first_request {
                    self.first_release.acquire().await.unwrap().forget();
                }
            } else {
                self.second_started.add_permits(1);
            }
            anyhow::ensure!(!self.fail, "injected bounded TTS failure");
            let result = budget.copy_from_slice(&[0; 400]).await;
            if call == 0 {
                self.first_completed.add_permits(1);
            }
            result
        }
        async fn synthesize_stream_request(
            &self,
            _: &crate::providers::adapter::TtsRequest,
            _: crate::providers::tts::audio_budget::ResponseAudioBudget,
        ) -> anyhow::Result<crate::providers::tts::audio_budget::TtsAudioStream> {
            unreachable!("fixture expects buffered synthesis")
        }
    }
    with_linked_fixture(|mut fixture| async move {
        let speech = Arc::new(Speech {
            calls: Default::default(),
            db: fixture.conversation.db.as_ref().unwrap().clone(),
            owners: Default::default(),
            texts: Default::default(),
            fail: fail_tts,
            pause_first_request,
            first_started: tokio::sync::Semaphore::new(0),
            first_release: tokio::sync::Semaphore::new(0),
            first_completed: tokio::sync::Semaphore::new(0),
            second_started: tokio::sync::Semaphore::new(0),
        });
        let llm = Arc::new(Streaming);
        let modules = Arc::get_mut(&mut fixture.modules).unwrap();
        modules.llm = llm.clone();
        modules.minor_llm = llm.clone();
        modules.major_llm = llm;
        modules.tts = speech.clone();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let output = Arc::new(OutputChannel {
            owner: Some(Uuid::from_u128(42)),
            streaming: true,
            first_gate: Some(gate.clone()),
            ..Default::default()
        });
        let channels = Arc::new(ChannelRegistry::new());
        channels.register(output.clone()).await.unwrap();
        let mut recognized = input("Read two sentences.");
        recognized.user_id = Uuid::from_u128(42).to_string();
        attach_output(&fixture.modules, output.clone(), &mut recognized);
        let control = recognized.output.as_ref().unwrap().output();
        let scope = recognized.output.as_ref().unwrap().scope();
        let modules = fixture.modules.clone();
        let policy = fixture.policy.clone();
        let task = tokio::spawn(async move {
            voice::process_recognized_voice(
                recognized,
                &modules,
                &channels,
                &EventBus::new(8),
                &VoiceTurnRegistry::default(),
                false,
                &policy,
            )
            .await
        });
        if pause_first_request {
            speech.first_started.acquire().await.unwrap().forget();
            output.controls.lock().unwrap().get_mut(&scope).unwrap().observe_capture(0, true, 0).unwrap();
            assert!(control.explicit_wait(Uuid::new_v4(), 0).unwrap());
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while output.control_sequence.load(Ordering::SeqCst) < 1 { tokio::task::yield_now().await; }
            }).await.unwrap();
            speech.first_release.add_permits(1);
            speech.first_completed.acquire().await.unwrap().forget();
            assert!(tokio::time::timeout(std::time::Duration::from_millis(100),
                speech.second_started.acquire()).await.is_err(), "Held started another TTS request");
            assert!(output.audio.lock().unwrap().is_empty(), "Held emitted first in-flight result");
            assert_eq!(speech.calls.load(Ordering::SeqCst), 1);
            assert!(control.request_resume(alia_core::playback_control::ResumeAuthorization {
                kind: alia_core::playback_control::ResumeKind::ExplicitContinue,
                detection_id: Uuid::new_v4(), through_sequence: 0,
            }).unwrap());
            tokio::time::timeout(std::time::Duration::from_secs(3), control.wait_open()).await.unwrap().unwrap();
        }
        if fail_tts {
            task.await.unwrap().unwrap();
            assert_eq!(*speech.texts.lock().unwrap(), [
                "First sentence.".to_owned(),
            ], "a failed bound voice scope must not create a replacement notice or replay the response");
            assert_eq!(speech.calls.load(Ordering::SeqCst), 1);
            assert!(output.audio.lock().unwrap().is_empty());
            let owners = speech.owners.lock().unwrap().clone();
            assert_eq!(owners.len(), 1, "a bound voice output retains its sole canonical Response");
            let id = owners[0];
            let response = fixture.modules.delivery.get_response(Uuid::from_u128(42), id).await.unwrap();
            assert_ne!(response.status, crate::modules::delivery::types::ResponseStatus::Committed);
            assert!(response.final_content.is_none());
            return;
        }
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while speech.calls.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("second sentence must synthesize while first playback remains gated");
        assert!(!task.is_finished());
        gate.add_permits(1);
        task.await.unwrap().unwrap();
        use sea_orm::ConnectionTrait;
        let db = fixture.conversation.db.as_ref().unwrap();
        let row = db
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT response_id FROM delivery_responses".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        let id: Uuid = row.try_get("", "response_id").unwrap();
        assert_eq!(*speech.owners.lock().unwrap(), [id, id]);
        assert!(output.audio.lock().unwrap().iter().filter(|payload| payload.data.owner().is_some())
            .all(|payload| payload.data.owner() == Some(crate::providers::tts::audio_budget::AudioBudgetOwner::Response(id))));
        let records = fixture
            .modules
            .delivery
            .list_deliveries(Uuid::from_u128(42), id)
            .await
            .unwrap();
        assert_eq!(records.len(), 3);
        let text_record = records
            .iter()
            .find(|record| {
                record.media_kind == crate::modules::delivery::types::DeliveryMedium::Text
            })
            .unwrap();
        assert_eq!(
            text_record.status,
            crate::modules::delivery::types::DeliveryStatus::Unknown
        );
        assert!(text_record.text.as_ref().unwrap().end_offered);
        let records = records
            .into_iter()
            .filter(|record| {
                record.media_kind == crate::modules::delivery::types::DeliveryMedium::Audio
            })
            .collect::<Vec<_>>();
        assert_eq!(records[0].output_key, "speech_sentence:0");
        assert_eq!(records[1].output_key, "speech_sentence:1");
        assert_ne!(
            records[0].identity.unwrap().playback_id,
            records[1].identity.unwrap().playback_id
        );
        assert!(
            records.iter().all(|r| r.status
                == crate::modules::delivery::types::DeliveryStatus::Submitted
                && r.presented_frames.is_none()),
            "{records:#?}"
        );
        let response = fixture
            .modules
            .delivery
            .get_response(Uuid::from_u128(42), id)
            .await
            .unwrap();
        assert_eq!(
            response.status,
            crate::modules::delivery::types::ResponseStatus::Committed
        );
        assert_eq!(
            response.final_content,
            Some(serde_json::json!({"text":"First sentence. Second sentence."}))
        );
    })
    .await;
}

fn stable_user_uuid(user_id: &str) -> Uuid {
    let user_id = user_id.trim();
    let stable_key = if user_id.is_empty() {
        "unknown-pipeline-user"
    } else {
        user_id
    };
    Uuid::parse_str(stable_key).unwrap_or_else(|_| {
        let digest = Sha256::digest(stable_key.as_bytes());
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x50;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid::from_bytes(bytes)
    })
}
