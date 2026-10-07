//! Cache answers pass through the same durable response and text-delivery exit.
use super::*;
use crate::modules::delivery::{
    repository::DeliveryRepository,
    types::{DeliveryMedium, NewResponse, ResponseKey, ResponseStatus},
};
use crate::modules::memory::context::{ContextEvidence, ContextService, ContextSource, ContextUse};
use crate::providers::adapter::{
    CACHE_METADATA_SCHEMA, CacheBackend, CacheCas, CacheEntry, CacheReview, CachedResponse,
    DEFAULT_CACHE_SCOPE, EmbeddingAdapter, EmbeddingProfile, EmbeddingVector, ProfiledEmbeddings,
    embedding_text_digest,
};
use crate::providers::cache::DbCache;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
use serde_json::json;

const QUERY: &str = "Explain a binary search.";
const ANSWER: &str = "Binary search repeatedly halves a sorted search interval.";

struct FixedEmbedding;
impl ModuleLifecycle for FixedEmbedding {
    fn name(&self) -> &'static str {
        "cache-exit-test-embedding"
    }
}
impl FixedEmbedding {
    fn profile() -> EmbeddingProfile {
        EmbeddingProfile {
            provider: "deterministic".into(),
            endpoint: "http://cache-exit.test".into(),
            model: "fixed-1536".into(),
            revision: "test.v1".into(),
            dimensions: 1536,
            distance: "cosine".into(),
            vector_type: "vector".into(),
        }
    }
    fn vector(text: &str) -> EmbeddingVector {
        EmbeddingVector {
            input_digest: embedding_text_digest(text),
            values: vec![1.0; 1536],
        }
    }
}
#[async_trait]
impl EmbeddingAdapter for FixedEmbedding {
    async fn embed(&self, _: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        panic!("semantic reuse must preserve the embedding profile")
    }
    async fn embed_profiled(&self, texts: &[String]) -> anyhow::Result<Option<ProfiledEmbeddings>> {
        Ok(Some(ProfiledEmbeddings {
            profile: Self::profile(),
            vectors: texts.iter().map(|text| Self::vector(text)).collect(),
        }))
    }
    fn dimension(&self) -> usize {
        1536
    }
}
fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}

async fn seed_cache(f: &mut Fixture, semantic: bool) -> (CachedResponse, Uuid) {
    let db = f.conversation.db.as_ref().unwrap();
    let owner = Uuid::from_u128(42);
    f.policy.cache.enabled = true;
    f.policy.cache.exact_match_enabled = true;
    f.policy.cache.semantic_match_enabled = semantic;
    let store = f.modules.context.policy_store();
    let current = store.current().await.unwrap();
    let mut policy = current.policy.with_cache(&f.policy.cache).unwrap();
    policy.cache_embedding_profile_id = Some(FixedEmbedding::profile().id().unwrap());
    store.replace(current.version, policy).await.unwrap();
    let cache = Arc::new(DbCache::with_database(db.clone()));
    let services = Arc::get_mut(&mut f.modules).unwrap();
    services.cache = cache.clone();
    services.embedding = Arc::new(FixedEmbedding);

    // The cached origin is a separate conversation, with a real source lease
    // and atomic Response/history commit. It cannot borrow the later request.
    let session = Uuid::new_v4();
    let source = Uuid::new_v4();
    let received_at =
        chrono::DateTime::from_timestamp_micros(chrono::Utc::now().timestamp_micros()).unwrap()
            - chrono::Duration::hours(2);
    db.execute(sql(
        "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'cache-origin',$2)",
        vec![session.into(), owner.to_string().into()],
    ))
    .await
    .unwrap();
    db.execute(sql(
        "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch,source_relationship_epoch) VALUES($1,$2,$3,'user',$4,$5,41,0)",
        vec![source.into(),session.into(),owner.to_string().into(),QUERY.into(),received_at.into()],
    )).await.unwrap();
    let generation = f
        .modules
        .context
        .materialize(
            ContextSource {
                owner_id: owner,
                request_id: Uuid::new_v4(),
                channel_id: "cache-origin".into(),
                conversation_id: Some(session),
                source_event_id: Some(source),
                source_received_at: received_at,
                source_privacy_epoch: 41,
            },
            QUERY,
        )
        .await
        .unwrap();
    let ledger = DeliveryRepository::new(db.clone());
    let origin = ledger
        .prepare(NewResponse {
            key: ResponseKey {
                user_id: owner,
                request_id: generation.source().request_id,
                output_key: "response".into(),
            },
            context_id: generation.lease().id(),
            conversation_id: Some(session),
            source_event_id: Some(source),
            source_received_at: received_at,
            source_privacy_epoch: Some(41),
            channel_id: "cache-origin".into(),
            kind: "assistant_response".into(),
            content: json!({"kind":"assistant_response"}),
        })
        .await
        .unwrap();
    let tx = db.begin().await.unwrap();
    let assistant = Uuid::new_v4();
    ledger
        .mark_generated_in_transaction(
            &tx,
            owner,
            origin.response_id,
            assistant,
            json!({"text":ANSWER}),
        )
        .await
        .unwrap();
    tx.execute(sql(
        "INSERT INTO chat_messages(id,session_id,role,content) VALUES($1,$2,'assistant',$3)",
        vec![assistant.into(), session.into(), ANSWER.into()],
    ))
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let pending = cache
        .put(CacheEntry {
            context: generation.lease().binding(),
            origin_response_id: origin.response_id,
            scope: DEFAULT_CACHE_SCOPE.into(),
            query_text: QUERY.into(),
            response_text: ANSWER.into(),
            metadata: json!({}),
            ttl_seconds: 3600,
        })
        .await
        .unwrap();
    let CacheCas::Applied(Some(active)) = cache.apply_review(&pending, CacheReview::Approve {
        response_text: ANSWER.into(),
        metadata: json!({"schema":CACHE_METADATA_SCHEMA,"policy":"private_exact",
            "safe_direct_return":true,"memory_independent":true,"tool_free":true,"dynamic_free":true}),
    }).await.unwrap() else { panic!("fresh cache review must apply"); };
    if semantic {
        assert_eq!(
            cache
                .fill_embedding(
                    &active,
                    &FixedEmbedding::profile(),
                    &FixedEmbedding::vector(QUERY)
                )
                .await
                .unwrap(),
            CacheCas::Applied(())
        );
    }
    (active, source)
}

#[tokio::test]
async fn exact_and_semantic_hits_share_one_response_exit_and_keep_revocable_source_lineage() {
    for semantic in [false, true] {
        with_linked_fixture(move |mut f| async move {
            let (cached, original_source) = seed_cache(&mut f, semantic).await;
            let owner = Uuid::from_u128(42);
            let query = if semantic { "How does binary search halve its interval?" } else { QUERY };
            let received_at = chrono::DateTime::from_timestamp_micros(chrono::Utc::now().timestamp_micros()).unwrap() - chrono::Duration::hours(1);
            process_event(
                ChannelEvent::TextMessage {
                    channel_id: "test".into(), user_id: owner.to_string(), text: query.into(),
                    source_received_at: received_at, source_privacy_epoch: 41, source_relationship_epoch: 0,
                },
                &f.modules, &f.channels, &f.sensors, &EventBus::new(32), CancellationToken::new(), false,
                &f.policy.conversation, &f.policy.tools, &f.policy.cache, &f.policy.cache_policy, &f.policy.memory,
            ).await.unwrap();
            assert!(f.started.try_recv().is_err(), "a safe cache hit called the response LLM");
            assert_eq!(*f.output.texts.lock().unwrap(), vec![ANSWER.to_string()]);
            assert_eq!(f.conversation.messages.lock().unwrap().iter().filter(|message| message.role == "assistant").count(), 1);
            let hit_count = f.conversation.events.lock().unwrap().iter().filter(|event|
                event.event_type == "cache_hit" && event.payload["kind"] == if semantic { "semantic" } else { "exact" }
            ).count();
            assert_eq!(hit_count, 1, "the requested cache branch was not used");

            let db = f.conversation.db.as_ref().unwrap();
            let responses = db.query_all(sql("SELECT response_id FROM delivery_responses WHERE channel_id='test'", vec![])).await.unwrap();
            assert_eq!(responses.len(), 1, "cache reuse duplicated its canonical Response");
            let response_id = responses[0].try_get("", "response_id").unwrap();
            let response = f.modules.delivery.get_response(owner, response_id).await.unwrap();
            assert_eq!(response.status, ResponseStatus::Committed);
            assert_eq!(response.final_content, Some(json!({"text":ANSWER})));
            assert_eq!(response.source_privacy_epoch, Some(41));
            assert_eq!(response.source_received_at, received_at);
            assert_ne!(response.context_id, Some(cached.version.context_id));
            let history = db.query_all(sql("SELECT id,content FROM chat_messages WHERE session_id=$1 AND role='assistant'", vec![response.conversation_id.into()])).await.unwrap();
            assert_eq!(history.len(), 1, "cache reuse duplicated assistant history");
            assert_eq!(Some(history[0].try_get::<Uuid>("", "id").unwrap()), response.assistant_message_id);
            assert_eq!(history[0].try_get::<String>("", "content").unwrap(), ANSWER);
            let deliveries = f.modules.delivery.list_deliveries(owner, response_id).await.unwrap();
            assert_eq!(deliveries.len(), 1);
            assert_eq!(deliveries[0].media_kind, DeliveryMedium::Text);
            assert!(deliveries[0].text.as_ref().unwrap().end_offered);

            let tx = db.begin().await.unwrap();
            let validated = ContextService::validate_lease_in_transaction(&tx, owner, response.context_id.unwrap(), ContextUse::ResponseCommit).await.unwrap();
            let dependencies = validated.lease.dependencies();
            assert!(dependencies.iter().any(|evidence| matches!(evidence, ContextEvidence::Cache { id, revision, context_id, .. }
                if *id == cached.version.cache_id && *revision == cached.version.revision && *context_id == cached.version.context_id)));
            for source in [original_source, response.source_event_id.unwrap()] {
                assert!(dependencies.iter().any(|evidence| matches!(evidence, ContextEvidence::UserMessage { id, .. } if *id == source)), "cache reuse lost a canonical source");
            }
            assert_eq!(validated.lease.source().request_id, response.key.request_id);
            let binding = validated.lease.binding();
            tx.commit().await.unwrap();
            assert_eq!(f.modules.cache.get(owner, DEFAULT_CACHE_SCOPE, QUERY).await.unwrap().unwrap().version, cached.version,
                "cache reuse rewrote the source cache entry");

            // Retire only the cache origin. A new request's otherwise valid
            // context must not launder the erased source into reusable output.
            db.execute(sql("DELETE FROM chat_messages WHERE id=$1", vec![original_source.into()])).await.unwrap();
            assert!(f.modules.context.validate(&binding, ContextUse::ResponseCommit).await.is_err());
            assert!(f.modules.cache.get(owner, DEFAULT_CACHE_SCOPE, QUERY).await.unwrap().is_none());
            if semantic {
                assert!(f.modules.cache.get_semantic_candidates(owner, DEFAULT_CACHE_SCOPE, &FixedEmbedding::profile(), &FixedEmbedding::vector(query), 3).await.unwrap().is_empty());
            }
        }).await;
    }
}

/// Block the selected cache read, making cancellation timing deterministic.
struct BlockedCache {
    entered: Arc<tokio::sync::Notify>,
    semantic: bool,
}
impl ModuleLifecycle for BlockedCache {
    fn name(&self) -> &'static str {
        "blocked-cache"
    }
}
#[async_trait]
impl CacheBackend for BlockedCache {
    async fn get(&self, _: Uuid, _: &str, _: &str) -> anyhow::Result<Option<CachedResponse>> {
        if self.semantic {
            return Ok(None);
        }
        self.entered.notify_one();
        std::future::pending().await
    }
    async fn put(&self, _: CacheEntry) -> anyhow::Result<CachedResponse> {
        panic!("cancelled input must not write cache")
    }
    async fn get_semantic_candidates(
        &self,
        _: Uuid,
        _: &str,
        _: &EmbeddingProfile,
        _: &EmbeddingVector,
        _: usize,
    ) -> anyhow::Result<Vec<crate::providers::adapter::SemanticCacheCandidate>> {
        assert!(self.semantic, "semantic cache is disabled in this case");
        self.entered.notify_one();
        std::future::pending().await
    }
    async fn load_pending(
        &self,
        _: Uuid,
        _: Option<&str>,
        _: usize,
    ) -> anyhow::Result<Vec<CachedResponse>> {
        panic!("request must not schedule cache review")
    }
    async fn apply_review(
        &self,
        _: &CachedResponse,
        _: CacheReview,
    ) -> anyhow::Result<CacheCas<Option<CachedResponse>>> {
        panic!("request must not apply cache review")
    }
    async fn authorize_work(
        &self,
        _: &CachedResponse,
        _: crate::providers::adapter::CacheWork,
    ) -> anyhow::Result<bool> {
        panic!("request must not authorize cache review")
    }
    async fn fill_embedding(
        &self,
        _: &CachedResponse,
        _: &EmbeddingProfile,
        _: &EmbeddingVector,
    ) -> anyhow::Result<CacheCas<()>> {
        panic!("cancelled input must not fill cache embedding")
    }
    async fn cleanup_expired(&self, _: usize) -> anyhow::Result<u64> {
        panic!("request must not run cache maintenance")
    }
}

#[tokio::test]
async fn cancelled_cache_lookup_preserves_user_source_and_terminal_audit() {
    for semantic in [false, true] {
        with_linked_fixture(move |mut f| async move {
            f.policy.cache.enabled = true;
            f.policy.cache.exact_match_enabled = true;
            f.policy.cache.semantic_match_enabled = semantic;
            let policy_store = f.modules.context.policy_store();
            let current = policy_store.current().await.unwrap();
            let mut policy = current.policy.with_cache(&f.policy.cache).unwrap();
            policy.cache_embedding_profile_id = Some(FixedEmbedding::profile().id().unwrap());
            policy_store.replace(current.version, policy).await.unwrap();
            let entered = Arc::new(tokio::sync::Notify::new());
            let services = Arc::get_mut(&mut f.modules).unwrap();
            services.cache = Arc::new(BlockedCache {
                entered: entered.clone(),
                semantic,
            });
            services.embedding = Arc::new(FixedEmbedding);
            let mut turn = VoiceTurn::request(CancellationToken::new());
            turn.output_channel = Some(f.output.clone());
            turn.source_privacy_epoch = Some(41);
            turn.source_relationship_epoch = Some(0);
            let owner = Uuid::from_u128(42);
            let user = owner.to_string();
            let target = ChannelTarget {
                user_id: user.clone(),
                voice: None,
            };
            let bus = EventBus::new(8);
            let request = process_text(
                QUERY,
                &user,
                &f.modules,
                f.output.as_ref(),
                &target,
                "test",
                &bus,
                false,
                false,
                false,
                &f.policy.conversation,
                &f.policy.tools,
                &f.policy.cache,
                &f.policy.cache_policy,
                &f.policy.memory,
                Some(&turn),
                true,
            );
            let cancel_after_lookup = async {
                entered.notified().await;
                turn.token.cancel();
            };
            let (result, ()) = tokio::join!(request, cancel_after_lookup);
            assert!(result.unwrap().interrupted);
            assert!(turn.response_id.get().is_none());
            assert!(
                f.started.try_recv().is_err(),
                "cancelled cache lookup reached the model"
            );
            assert!(f.output.texts.lock().unwrap().is_empty());
            assert!(f.output.audio.lock().unwrap().is_empty());
            let messages = f.conversation.messages.lock().unwrap();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].role, "user");
            assert_eq!(messages[0].content, QUERY);
            drop(messages);
            let events = f.conversation.events.lock().unwrap();
            let terminal = events
                .iter()
                .filter(|event| event.turn_id == turn.id && event.sequence == i32::MAX)
                .collect::<Vec<_>>();
            assert_eq!(
                terminal.len(),
                1,
                "cache cancellation lost the terminal interruption audit"
            );
            assert_eq!(terminal[0].event_type, "turn_interrupted");
            let expected_stage = if semantic {
                "semantic_cache_candidates"
            } else {
                "cache_lookup"
            };
            assert_eq!(terminal[0].payload["stage"], expected_stage);
            assert_eq!(terminal[0].payload["preserved_user_context"], true);
            assert_eq!(terminal[0].payload["assistant_response_persisted"], false);
            assert_eq!(terminal[0].payload["cache_written"], false);
            assert_eq!(terminal[0].payload["memory_capture"], false);
            assert!(events.iter().any(|event| event.turn_id == turn.id
                && event.sequence != i32::MAX
                && event.event_type == "turn_interrupted"
                && event.payload["stage"] == expected_stage));
            drop(events);
            let db = f.conversation.db.as_ref().unwrap();
            let source = turn.source_event_id.get().unwrap();
            assert_eq!(
                db.query_all(sql(
                    "SELECT id FROM chat_messages WHERE id=$1 AND role='user'",
                    vec![(*source).into()]
                ))
                .await
                .unwrap()
                .len(),
                1
            );
            assert!(
                db.query_all(sql(
                    "SELECT response_id FROM delivery_responses WHERE channel_id='test'",
                    vec![]
                ))
                .await
                .unwrap()
                .is_empty()
            );
        })
        .await;
    }
}
