use super::*;
use crate::modules::platform::lifecycle::ModuleLifecycle;
use sea_orm::ConnectionTrait;

fn owned_turn(fixture: &Fixture, epoch: i64) -> VoiceTurn {
    let mut turn = VoiceTurn::request(CancellationToken::new());
    turn.output_channel = Some(fixture.output.clone());
    turn.source_privacy_epoch = Some(epoch);
    turn.source_relationship_epoch = Some(0);
    turn
}

async fn generate(fixture: &Fixture, turn: &VoiceTurn, text: &str) -> anyhow::Result<TextResult> {
    let user = Uuid::from_u128(42).to_string();
    process_text(
        text,
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
        true,
        &fixture.policy.conversation,
        &fixture.policy.tools,
        &fixture.policy.cache,
        &fixture.policy.cache_policy,
        &fixture.policy.memory,
        Some(turn),
        false,
    )
    .await
}

#[tokio::test]
async fn corrected_transcript_keeps_the_single_persisted_raw_source_and_lease() {
    with_linked_fixture(|mut fixture| async move {
        let turn = owned_turn(&fixture, 41);
        let raw = "turn on teh light";
        let user = Uuid::from_u128(42).to_string();
        let initial = ensure_turn_context(&fixture.modules, &turn, "test", &user, raw)
            .await
            .unwrap();
        let result = generate(&fixture, &turn, "turn on the light")
            .await
            .unwrap();
        assert_eq!(result.text, "recognized reply");
        assert_eq!(fixture.started.try_recv().unwrap(), "turn on the light");
        let state = turn.context.lock().await;
        assert!(state.frozen);
        assert_eq!(state.lease().unwrap().source(), initial.source());
        assert!(
            initial.lease().dependencies().iter().all(|root| state
                .lease()
                .unwrap()
                .dependencies()
                .contains(root))
        );
        assert!(state.generation.as_ref().unwrap().affect().is_some());
        drop(state);
        let messages = fixture.conversation.messages.lock().unwrap();
        let users: Vec<_> = messages
            .iter()
            .filter(|message| message.role == "user")
            .collect();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].content, raw);
        assert_eq!(fixture.conversation.source_times.lock().unwrap().len(), 1);
        assert!(turn.response_id.get().is_some());
    })
    .await;
}

struct DelayedLlm {
    entered: Arc<tokio::sync::Semaphore>,
    release: Arc<tokio::sync::Semaphore>,
    block_once: AtomicBool,
    prompts: Mutex<Vec<Vec<ChatMessage>>>,
}
impl ModuleLifecycle for DelayedLlm {
    fn name(&self) -> &'static str {
        "context_barrier_llm"
    }
}
#[async_trait]
impl LlmAdapter for DelayedLlm {
    async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String> {
        self.prompts.lock().unwrap().push(messages.to_vec());
        if self.block_once.swap(false, Ordering::SeqCst) {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        Ok("A generated reply from this context.".into())
    }
    async fn chat_stream(
        &self,
        _: &[ChatMessage],
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> {
        anyhow::bail!("barrier test must use buffered generation")
    }
}

#[tokio::test]
async fn clear_or_policy_change_during_model_call_rejects_old_output_and_allows_a_new_turn() {
    for clear in [true, false] {
        with_linked_fixture(move |mut fixture| async move {
            let llm = Arc::new(DelayedLlm {
                entered: Arc::new(tokio::sync::Semaphore::new(0)),
                release: Arc::new(tokio::sync::Semaphore::new(0)),
                block_once: AtomicBool::new(true),
                prompts: Mutex::default(),
            });
            let modules = Arc::get_mut(&mut fixture.modules).unwrap();
            modules.llm = llm.clone();
            modules.minor_llm = llm.clone();
            modules.major_llm = llm.clone();
            let turn = owned_turn(&fixture, 41);
            let generation = generate(&fixture, &turn, "This old source must not be reintroduced.");
            let revoke = async {
                llm.entered.acquire().await.unwrap().forget();
                if clear {
                    let memory = crate::modules::memory::MemoryService::new();
                    memory
                        .attach_runtime_for_test(
                            fixture.conversation.db.as_ref().unwrap().clone(),
                            None,
                            Default::default(),
                        )
                        .await
                        .unwrap();
                    memory.clear(Uuid::from_u128(42)).await.unwrap();
                } else {
                    let store = fixture.modules.context.policy_store();
                    let mut current = store.current().await.unwrap();
                    current.policy.max_memory_chars += 1;
                    store
                        .replace(current.version, current.policy)
                        .await
                        .unwrap();
                }
                llm.release.add_permits(1);
            };
            let (old, ()) = tokio::join!(generation, revoke);
            assert!(old.is_err(), "late model output reused revoked context");
            assert!(turn.response_id.get().is_none());
            assert!(fixture.output.texts.lock().unwrap().is_empty());
            assert!(
                !fixture
                    .conversation
                    .messages
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|message| message.role == "assistant")
            );
            let next = owned_turn(&fixture, if clear { 42 } else { 41 });
            let new = generate(&fixture, &next, "This is a new source.")
                .await
                .unwrap();
            assert_eq!(new.text, "A generated reply from this context.");
            if clear {
                let prompts = llm.prompts.lock().unwrap();
                assert!(
                    !prompts
                        .last()
                        .unwrap()
                        .iter()
                        .any(|message| message.content.contains("old source"))
                );
            }
            let db = fixture.conversation.db.as_ref().unwrap();
            let count: i64 = db
                .query_one(sea_orm::Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT count(*) AS count FROM delivery_responses WHERE status='committed'",
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "count")
                .unwrap();
            assert_eq!(count, 1);
        })
        .await;
    }
}
