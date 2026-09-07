//! Tests the actual ingress/response chain with in-memory collaborators. No
//! model endpoint or database is contacted; STT panics if called unexpectedly.
use super::*;
use crate::module::{ModuleLifecycle, factory};
use crate::providers::adapter::{
    ConversationBackend, RouterStrategy, SttAdapter, TranscribeChunk, TranscriptionResult,
};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;
use std::sync::Mutex;
use tokio::sync::mpsc;

#[derive(Default)]
struct Conversation {
    messages: Mutex<Vec<ChatMessage>>,
    events: Mutex<Vec<ConversationEventEntry>>,
}
impl ModuleLifecycle for Conversation {
    fn name(&self) -> &'static str {
        "test_conversation"
    }
}
#[async_trait]
impl ConversationBackend for Conversation {
    async fn get_or_create_active_session(&self, _: &str, _: &str) -> anyhow::Result<Uuid> {
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
        self.messages.lock().unwrap().push(message);
        Ok(())
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
        "test"
    }
    fn capabilities(&self) -> crate::channels::adapter::ChannelCapabilities {
        crate::channels::adapter::ChannelCapabilities {
            text: true,
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
struct Fixture {
    modules: Arc<ModuleRegistry>,
    channels: Arc<ChannelRegistry>,
    sensors: Arc<SensorRegistry>,
    output: Arc<OutputChannel>,
    conversation: Arc<Conversation>,
    policy: PipelinePolicies,
    started: mpsc::UnboundedReceiver<String>,
}
impl Fixture {
    async fn new(block: bool) -> Self {
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
            llm: llm.clone(),
            minor_llm: llm.clone(),
            major_llm: llm,
            transcript_correction_llm: None,
            memory_capture_llm: None,
            sentence_expression_llm: None,
            stt: Arc::new(UnexpectedStt),
            tts: factory::create_tts("noop").into(),
            embedding: factory::create_embedding(&config, "noop").unwrap().into(),
            conversation: conversation.clone(),
            memory: Arc::new(crate::providers::memory::NoopMemory),
            cache: factory::create_cache().into(),
            skills: factory::create_skills().into(),
            persona: factory::create_persona().into(),
            insights: factory::create_insights(false).into(),
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
async fn recognized_speech_reaches_llm_and_shared_text_tts_without_stt() {
    let mut fixture = Fixture::new(false).await;
    let input = input("Keep this exact recognized transcript.");
    let session_id = input.session_id;
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
}

#[tokio::test]
async fn saturated_voice_queue_rejects_overflow_and_skips_cancelled_input() {
    let mut fixture = Fixture::new(true).await;
    let (ingress, receiver) = crate::modules::interaction::voice_input::channel();
    let shutdown = CancellationToken::new();
    let pipeline = spawn_pipeline(
        &EventBus::new(8),
        fixture.modules,
        fixture.channels,
        fixture.sensors,
        shutdown.clone(),
        false,
        Arc::new(RwLock::new(fixture.policy)),
        receiver,
    );
    let mut active_tokens = Vec::new();
    for index in 0..supervision::MAX_CONCURRENT_REQUESTS {
        let input = input(&format!("running-{index}"));
        active_tokens.push(input.cancel.clone());
        ingress.try_submit(input).unwrap();
    }
    for _ in 0..supervision::MAX_CONCURRENT_REQUESTS {
        tokio::time::timeout(std::time::Duration::from_secs(2), fixture.started.recv())
            .await
            .unwrap()
            .unwrap();
    }
    let stale = input("cancelled-queued");
    stale.cancel.cancel();
    ingress.try_submit(stale).unwrap();
    for index in 0..7 {
        ingress
            .try_submit(input(&format!("queued-{index}")))
            .unwrap();
    }
    assert!(matches!(
        ingress.try_submit(input("overflow")),
        Err(mpsc::error::TrySendError::Full(_))
    ));
    active_tokens[0].cancel();
    let next = tokio::time::timeout(std::time::Duration::from_secs(2), fixture.started.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next, "queued-0");
    shutdown.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), pipeline)
        .await
        .unwrap()
        .unwrap();
    assert!(
        fixture.started.try_recv().is_err(),
        "queued work started after shutdown"
    );
    assert!(fixture.output.texts.lock().unwrap().is_empty());
    assert!(fixture.output.audio.lock().unwrap().is_empty());
    assert!(matches!(
        ingress.try_submit(input("closed")),
        Err(mpsc::error::TrySendError::Closed(_))
    ));
}
