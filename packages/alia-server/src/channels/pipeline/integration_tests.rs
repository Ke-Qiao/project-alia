//! Tests the ingress/response chain with controlled model and media collaborators.
//! Successful delivery paths use real PostgreSQL; unexpected STT calls panic.
use super::*;
use crate::module::{ModuleLifecycle, factory};
use crate::providers::adapter::{
    ConversationBackend, RouterStrategy, SttAdapter, TranscribeChunk, TranscriptionResult,
};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
mod context;
mod memory_search;
mod text_delivery;

#[derive(Default)]
struct Conversation {
    messages: Mutex<Vec<ChatMessage>>,
    events: Mutex<Vec<ConversationEventEntry>>,
    source_times: Mutex<Vec<chrono::DateTime<chrono::Utc>>>,
    source_epochs: Mutex<Vec<i64>>,
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
    ) -> anyhow::Result<Uuid> {
        anyhow::ensure!(
            !self.fail_user.load(Ordering::SeqCst),
            "injected source persistence failure"
        );
        anyhow::ensure!(message.role == "user", "source must be user input");
        let source = Uuid::new_v4();
        if let Some(db) = &self.db {
            use sea_orm::ConnectionTrait;
            db.execute(sea_orm::Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,"INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user',$4,$5,$6)",vec![source.into(),session.into(),owner.to_string().into(),message.content.clone().into(),received_at.into(),source_privacy_epoch.into()])).await?;
        }
        self.append_message(session, message).await?;
        self.source_times.lock().unwrap().push(received_at);
        self.source_epochs
            .lock()
            .unwrap()
            .push(source_privacy_epoch);
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
struct UnexpectedStt;
impl ModuleLifecycle for UnexpectedStt {
    fn name(&self) -> &'static str {
        "unexpected_stt"
    }
}
#[async_trait]
impl SttAdapter for UnexpectedStt {
    async fn transcribe_result(
        &self,
        _: &[u8],
        _: AudioEncoding,
    ) -> anyhow::Result<TranscriptionResult> {
        panic!("recognized voice must never be transcribed again");
    }
    async fn transcribe_stream(
        &self,
        _: Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
        _: AudioEncoding,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<TranscribeChunk>> + Send>>> {
        panic!("recognized voice must never be transcribed again");
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
    texts: Mutex<Vec<String>>,
    audio: Mutex<Vec<AudioPayload>>,
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
    fn reserve_playback(
        &self,
        target: &ChannelTarget,
        playback_id: Uuid,
    ) -> anyhow::Result<Option<alia_core::delivery::PlaybackIdentity>> {
        Ok(target
            .voice
            .map(|scope| alia_core::delivery::PlaybackIdentity {
                scope,
                playback_id,
                playback_sequence: self.sequence.fetch_add(1, Ordering::SeqCst) + 1,
            }))
    }
    async fn deliver_audio(
        &self,
        target: &ChannelTarget,
        identity: Option<alia_core::delivery::PlaybackIdentity>,
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
            self.send_audio(target, payload).await?;
        }
        if identity.unwrap().playback_sequence == 1 {
            if let Some(gate) = &self.first_gate {
                gate.acquire().await.unwrap().forget();
            }
        }
        let progress = alia_core::delivery::PlaybackProgress {
            identity: identity.unwrap(),
            receipt_sequence: 1,
            accepted_source_frames: frames,
            submitted_source_frames: Some(frames),
            presented_source_frames: None,
            end_accepted: true,
            backend: Some(alia_core::delivery::SubmissionBackend {
                instance_id: Uuid::new_v4(),
                boundary: alia_core::delivery::SubmissionBoundary::CpalCallbackBuffer,
                output_sample_rate_hz: 24000,
                output_channels: 1,
            }),
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
    async fn send_message(&self, _: &ChannelTarget, text: &str) -> anyhow::Result<()> {
        self.texts.lock().unwrap().push(text.to_owned());
        Ok(())
    }
    async fn send_audio(&self, _: &ChannelTarget, audio: AudioPayload) -> anyhow::Result<()> {
        self.audio.lock().unwrap().push(audio);
        Ok(())
    }
}
pub(super) struct Fixture {
    pub(super) modules: Arc<ModuleRegistry>,
    channels: Arc<ChannelRegistry>,
    sensors: Arc<SensorRegistry>,
    output: Arc<OutputChannel>,
    conversation: Arc<Conversation>,
    policy: PipelinePolicies,
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
        let modules = Arc::new(ModuleRegistry {
            context: Arc::new(crate::modules::memory::context::ContextService::new(
                sea_orm::DatabaseConnection::Disconnected,
                factory::create_embedding(&config, "noop").unwrap().into(),
            )),
            delivery: crate::modules::delivery::DeliveryService::new(
                crate::modules::delivery::repository::DeliveryRepository::new(
                    sea_orm::DatabaseConnection::Disconnected,
                ),
                &CancellationToken::new(),
            ),
            cleanup: crate::module::lifecycle::RuntimeCleanup::new(
                tokio_util::sync::CancellationToken::new(),
                std::time::Duration::from_secs(1),
            ),
            llm: llm.clone(),
            minor_llm: llm.clone(),
            major_llm: llm,
            transcript_correction_llm: None,
            memory_capture_llm: None,
            sentence_expression_llm: None,
            stt: Arc::new(UnexpectedStt),
            tts: factory::create_tts("noop").unwrap().into(),
            embedding: factory::create_embedding(&config, "noop").unwrap().into(),
            conversation: conversation.clone(),
            memory: Arc::new(crate::providers::memory::NoopMemory),
            cache: factory::create_cache().into(),
            skills: factory::create_skills().into(),
            persona: factory::create_persona().into(),
            insights: factory::create_insights(false).unwrap().into(),
            router: Arc::new(DirectRouter),
            rule_engine: factory::create_rule_engine().into(),
            scheduler: factory::create_scheduler(Arc::default(), Arc::default()).into(),
            tools: Arc::new(crate::tools::BuiltinToolRegistry::new()),
            expression: Arc::new(crate::expression::ExpressionRuntime::from_config(&config)),
            extensions: vec![],
        });
        let mut policy = PipelinePolicies::new(
            Default::default(),
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
        source_received_at: chrono::Utc::now() - chrono::Duration::hours(1),
        source_privacy_epoch: 41,
        channel_id: "test".into(),
        user_id: text.into(),
        transcript: text.into(),
        session_id: Uuid::new_v4(),
        device_boot_id: Uuid::new_v4(),
        generation_epoch: 7,
        cancel: CancellationToken::new(),
    }
}

#[tokio::test]
async fn clarification_session_failure_sends_no_unrecorded_text_or_audio() {
    let fixture = Fixture::new(false).await;
    fixture
        .conversation
        .fail_session
        .store(true, Ordering::SeqCst);
    let correction: TranscriptCorrectionResult = serde_json::from_value(serde_json::json!({
        "raw_text":"unclear target", "corrected_text":"unclear target", "confidence":0.5,
        "changed":false,"requires_confirmation":true,"risk":"medium","status":"unchanged","provider":"test"
    })).unwrap();
    handle_transcript_correction_confirmation(
        &fixture.modules,
        fixture.output.as_ref(),
        &ChannelTarget {
            user_id: Uuid::new_v4().to_string(),
            voice: None,
        },
        "test",
        "unlinked",
        "Which device?",
        &correction,
        false,
        None,
    )
    .await;
    assert!(fixture.output.texts.lock().unwrap().is_empty());
    assert!(fixture.output.audio.lock().unwrap().is_empty());
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
        None,
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
        let pipeline = spawn_pipeline(
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
        tokio::time::timeout(std::time::Duration::from_secs(15), Box::pin(test(fixture))).await
    }))
    .catch_unwind()
    .await;
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
    }
    impl ModuleLifecycle for Speech {
        fn name(&self) -> &'static str {
            "speech-test"
        }
    }
    #[async_trait]
    impl crate::providers::adapter::TtsAdapter for Speech {
        async fn synthesize(&self, _: &str) -> anyhow::Result<Vec<u8>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![0; 400])
        }
        async fn synthesize_stream(
            &self,
            _: &str,
        ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<Vec<u8>>> + Send>>> {
            panic!("sentence pipeline uses bounded pre-synthesis")
        }
    }
    with_linked_fixture(|mut fixture| async move {
        let speech = Arc::new(Speech {
            calls: Default::default(),
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
