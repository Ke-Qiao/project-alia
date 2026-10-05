//! Real PostgreSQL tests. Every case uses its own schema in the disposable
//! ALIA_TEST_DATABASE_URL database; no mock database or missing-table fallback.
use alia_server::{
    config::{ConversationSection, MemorySection},
    modules::{
        memory::context::{
            ContextPolicy, ContextService, ContextSource, ContextUse, GenerationContext,
        },
        platform::migrations,
    },
    providers::{adapter::*, cache::DbCache, embedding::NoopEmbedding},
};
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
    TransactionTrait,
};
use serde_json::{Value, json};
use std::{future::Future, panic::AssertUnwindSafe, sync::Arc};
use uuid::Uuid;

static CASES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

#[derive(Clone)]
struct Fixture {
    admin: DatabaseConnection,
    db: DatabaseConnection,
    cache: Arc<DbCache>,
    context: Arc<ContextService>,
    user: Uuid,
    schema: String,
    url: String,
}
fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}
fn profile(dimensions: usize) -> EmbeddingProfile {
    EmbeddingProfile {
        provider: "deterministic".into(),
        endpoint: "http://embedding.test".into(),
        model: format!("test-{dimensions}"),
        revision: "test.v1".into(),
        dimensions,
        distance: "cosine".into(),
        vector_type: "vector".into(),
    }
}
fn vector(profile: &EmbeddingProfile, query: &str) -> EmbeddingVector {
    EmbeddingVector {
        input_digest: embedding_text_digest(query),
        values: vec![1.0; profile.dimensions],
    }
}
fn approved_metadata() -> Value {
    json!({"schema":CACHE_METADATA_SCHEMA,"policy":"private_exact",
        "safe_direct_return":true,"memory_independent":true,"tool_free":true,"dynamic_free":true})
}
impl Fixture {
    async fn new() -> Self {
        let url =
            std::env::var("ALIA_TEST_DATABASE_URL").expect("set disposable ALIA_TEST_DATABASE_URL");
        let mut options = ConnectOptions::new(url.clone());
        options.max_connections(1).sqlx_logging(false);
        let admin = Database::connect(options)
            .await
            .expect("PostgreSQL must be reachable");
        let schema = format!("cache_context_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut options = ConnectOptions::new(url.clone());
        options
            .max_connections(5)
            .sqlx_logging(false)
            .set_schema_search_path(format!("{schema},public"));
        let db = Database::connect(options).await.unwrap();
        let context = Arc::new(ContextService::new(db.clone(), Arc::new(NoopEmbedding)));
        let f = Self {
            admin,
            cache: Arc::new(DbCache::with_database(db.clone())),
            db,
            context,
            user: Uuid::new_v4(),
            schema,
            url,
        };
        if let Err(error) = migrations::run(&f.db).await {
            f.cleanup().await;
            panic!("cache migrations failed: {error:#}");
        }
        let mut policy =
            ContextPolicy::from_config(&MemorySection::default(), &ConversationSection::default())
                .unwrap();
        policy.memory_enabled = false;
        policy.cache_enabled = true;
        policy.cache_exact_enabled = true;
        policy.cache_semantic_enabled = true;
        policy.cache_embedding_profile_id = Some(profile(1536).id().unwrap());
        f.context.initialize_policy(policy).await.unwrap();
        f.add_owner(f.user).await;
        f
    }
    async fn add_owner(&self, owner: Uuid) {
        self.db.execute(sql("INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'cache test','not-a-login','user','active')",
            vec![owner.into(),format!("{owner}@cache.test").into()])).await.unwrap();
        alia_server::modules::memory::source::capture_source_epoch(&self.db, owner)
            .await
            .unwrap();
    }
    async fn generation(&self, owner: Uuid, query: &str) -> GenerationContext {
        let epoch = alia_server::modules::memory::source::capture_source_epoch(&self.db, owner)
            .await
            .unwrap();
        let session = Uuid::new_v4();
        let message = Uuid::new_v4();
        self.db
            .execute(sql(
                "INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'cache-test',$2)",
                vec![session.into(), owner.to_string().into()],
            ))
            .await
            .unwrap();
        let row=self.db.query_one(sql(
            "INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user',$4,clock_timestamp(),$5) RETURNING source_received_at",
            vec![message.into(),session.into(),owner.to_string().into(),query.into(),epoch.into()],
        )).await.unwrap().unwrap();
        self.context
            .materialize(
                ContextSource {
                    owner_id: owner,
                    request_id: Uuid::new_v4(),
                    channel_id: "cache-test".into(),
                    conversation_id: Some(session),
                    source_event_id: Some(message),
                    source_received_at: row.try_get("", "source_received_at").unwrap(),
                    source_privacy_epoch: epoch,
                },
                query,
            )
            .await
            .unwrap()
    }
    async fn entry_from(
        &self,
        generation: &GenerationContext,
        scope: &str,
        query: &str,
        answer: &str,
    ) -> CacheEntry {
        let response = Uuid::new_v4();
        let assistant = Uuid::new_v4();
        let source = generation.source();
        self.db.execute(sql(
            "INSERT INTO chat_messages(id,session_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,'assistant',$3,$4,$5)",
            vec![assistant.into(),source.conversation_id.into(),answer.into(),source.source_received_at.into(),source.source_privacy_epoch.into()],
        )).await.unwrap();
        self.db.execute(sql(
            "INSERT INTO delivery_responses(response_id,user_id,request_id,output_key,conversation_id,assistant_message_id,source_event_id,source_received_at,source_privacy_epoch,channel_id,kind,content,final_content,intent_digest,status,context_id) VALUES($1,$2,$3,'answer',$4,$5,$6,$7,$8,'cache-test','conversation',$9,$9,$10,'committed',$11)",
            vec![response.into(),source.owner_id.into(),source.request_id.into(),source.conversation_id.into(),
                assistant.into(),source.source_event_id.into(),source.source_received_at.into(),
                source.source_privacy_epoch.into(),json!({"text":answer}).into(),
                embedding_text_digest(answer).into(),generation.lease().id().into()],
        )).await.unwrap();
        CacheEntry {
            context: generation.lease().binding(),
            origin_response_id: response,
            scope: scope.into(),
            query_text: query.into(),
            response_text: answer.into(),
            metadata: json!({}),
            ttl_seconds: 3600,
        }
    }
    async fn entry(&self, owner: Uuid, scope: &str, query: &str, answer: &str) -> CacheEntry {
        let context = self.generation(owner, query).await;
        self.entry_from(&context, scope, query, answer).await
    }
    async fn activate(&self, entry: CacheEntry) -> CachedResponse {
        let pending = self.cache.put(entry).await.unwrap();
        let CacheCas::Applied(Some(active)) = self
            .cache
            .apply_review(
                &pending,
                CacheReview::Approve {
                    response_text: pending.response_text.clone(),
                    metadata: approved_metadata(),
                },
            )
            .await
            .unwrap()
        else {
            panic!("fresh review must apply")
        };
        active
    }
    async fn set_profile(&self, profile: &EmbeddingProfile) {
        let current = self.context.policy_store().current().await.unwrap();
        let mut policy = current.policy;
        policy.cache_embedding_profile_id = Some(profile.id().unwrap());
        self.context
            .policy_store()
            .replace(current.version, policy)
            .await
            .unwrap();
    }
    async fn count(&self, table: &str) -> i64 {
        assert!(table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        self.db
            .query_one(sql(&format!("SELECT count(*) AS n FROM {table}"), vec![]))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap()
    }
    async fn cleanup(self) {
        self.db.close().await.unwrap();
        self.admin
            .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .await
            .unwrap();
        self.admin.close().await.unwrap();
    }
}
async fn with_db<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    let _permit = CASES.acquire().await.unwrap();
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(test(f.clone())).catch_unwind().await;
    f.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn atomic_replacement_and_repeated_origin_keep_one_monotonic_version() {
    with_db(|f| async move {
        let a = f
            .entry(f.user, "default", "same question", "first answer")
            .await;
        let b = f
            .entry(f.user, "default", "same question", "second answer")
            .await;
        let (a_result, b_result) = tokio::join!(f.cache.put(a), f.cache.put(b));
        let a_result = a_result.unwrap();
        let b_result = b_result.unwrap();
        assert_eq!(a_result.version.cache_id, b_result.version.cache_id);
        let mut revisions = vec![a_result.version.revision, b_result.version.revision];
        revisions.sort();
        assert_eq!(revisions, vec![1, 2]);
        assert_eq!(f.count("llm_cache").await, 1);
        let entry = f
            .entry(f.user, "default", "retry query", "retry answer")
            .await;
        let pending = f.cache.put(entry.clone()).await.unwrap();
        let CacheCas::Applied(Some(active)) = f
            .cache
            .apply_review(
                &pending,
                CacheReview::Approve {
                    response_text: "refined answer".into(),
                    metadata: approved_metadata(),
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        let repeated = f.cache.put(entry).await.unwrap();
        assert_eq!(repeated.version, active.version);
        assert_eq!(repeated.response_text, "refined answer");
        assert_eq!(repeated.status, CacheStatus::Active);
    })
    .await;
}

#[tokio::test]
async fn stale_review_and_embedding_cannot_modify_replacement_or_recreated_key() {
    with_db(|f| async move {
        let first = f.entry(f.user, "default", "query", "old").await;
        let old_pending = f.cache.put(first).await.unwrap();
        let replacement = f.entry(f.user, "default", "query", "new").await;
        let active = f.activate(replacement).await;
        assert!(matches!(
            f.cache
                .apply_review(&old_pending, CacheReview::Reject)
                .await
                .unwrap(),
            CacheCas::Stale
        ));
        assert!(matches!(
            f.cache
                .apply_review(
                    &old_pending,
                    CacheReview::Approve {
                        response_text: "stale mutation".into(),
                        metadata: approved_metadata()
                    }
                )
                .await
                .unwrap(),
            CacheCas::Stale
        ));
        let p = profile(1536);
        let old_vector = vector(&p, "query");
        assert_eq!(
            f.cache
                .fill_embedding(&old_pending, &p, &old_vector)
                .await
                .unwrap(),
            CacheCas::Stale
        );
        assert_eq!(
            f.cache
                .fill_embedding(&active, &p, &old_vector)
                .await
                .unwrap(),
            CacheCas::Applied(())
        );
        assert_eq!(
            f.cache
                .get(f.user, "default", "query")
                .await
                .unwrap()
                .unwrap()
                .response_text,
            "new"
        );
        f.db.execute(sql(
            "DELETE FROM llm_cache WHERE id=$1",
            vec![active.version.cache_id.into()],
        ))
        .await
        .unwrap();
        let rebuilt = f
            .activate(f.entry(f.user, "default", "query", "rebuilt").await)
            .await;
        assert_ne!(active.version.cache_id, rebuilt.version.cache_id);
        assert_eq!(
            f.cache
                .fill_embedding(&active, &p, &old_vector)
                .await
                .unwrap(),
            CacheCas::Stale
        );
        assert_eq!(f.count("llm_cache_embeddings").await, 0);
        assert_eq!(
            f.cache
                .fill_embedding(&rebuilt, &p, &old_vector)
                .await
                .unwrap(),
            CacheCas::Applied(())
        );
    })
    .await;
}

#[tokio::test]
async fn private_owner_scope_and_real_1536_vector_reads_are_isolated() {
    with_db(|f| async move {
        let other = Uuid::new_v4();
        f.add_owner(other).await;
        let p = profile(1536);
        for (owner, scope, answer) in [
            (f.user, "default", "mine"),
            (f.user, "other", "other scope"),
            (other, "default", "other owner"),
        ] {
            let active = f
                .activate(f.entry(owner, scope, "same", answer).await)
                .await;
            assert_eq!(
                f.cache
                    .fill_embedding(&active, &p, &vector(&p, "same"))
                    .await
                    .unwrap(),
                CacheCas::Applied(())
            );
        }
        assert_eq!(
            f.cache
                .get(f.user, "default", "same")
                .await
                .unwrap()
                .unwrap()
                .response_text,
            "mine"
        );
        assert_eq!(
            f.cache
                .get(f.user, "other", "same")
                .await
                .unwrap()
                .unwrap()
                .response_text,
            "other scope"
        );
        let candidates = f
            .cache
            .get_semantic_candidates(f.user, "default", &p, &vector(&p, "near"), 10)
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].cached.response_text, "mine");
        assert_eq!(candidates[0].cached.owner_id, f.user);
        assert!(f.cache.get(Uuid::nil(), "default", "same").await.is_err());
        assert!(f.cache.load_pending(Uuid::nil(), None, 20).await.is_err());
        assert!(
            f.db.execute_unprepared("UPDATE llm_cache SET user_id=NULL")
                .await
                .is_err()
        );
        f.db.execute(sql(
            "UPDATE users SET status='disabled' WHERE id=$1",
            vec![f.user.into()],
        ))
        .await
        .unwrap();
        assert!(f.cache.get(f.user, "default", "same").await.is_err());
        assert!(
            f.cache
                .get_semantic_candidates(f.user, "default", &p, &vector(&p, "near"), 3)
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn committed_response_context_owner_and_original_body_are_required() {
    with_db(|f| async move {
        let valid = f
            .entry(f.user, "default", "question", "canonical answer")
            .await;
        let mut changed = valid.clone();
        changed.response_text = "not the committed response".into();
        assert!(f.cache.put(changed).await.is_err());
        let mut missing = valid.clone();
        missing.origin_response_id = Uuid::new_v4();
        assert!(f.cache.put(missing).await.is_err());
        let other = Uuid::new_v4();
        f.add_owner(other).await;
        let mut wrong_owner = valid.clone();
        wrong_owner.context.owner_id = other;
        assert!(f.cache.put(wrong_owner).await.is_err());
        let another = f.generation(f.user, "another question").await;
        let mut wrong_context = valid.clone();
        wrong_context.context = another.lease().binding();
        assert!(f.cache.put(wrong_context).await.is_err());
        assert_eq!(f.count("llm_cache").await, 0);
        f.db.execute(sql(
            "UPDATE delivery_responses SET cancel_requested=TRUE WHERE response_id=$1",
            vec![valid.origin_response_id.into()],
        ))
        .await
        .unwrap();
        assert!(f.cache.put(valid).await.is_err());
    })
    .await;
}

#[tokio::test]
async fn profile_changes_revoke_late_work_and_dimensions_never_mix() {
    with_db(|f| async move {
        let p = profile(1536);
        let active = f
            .activate(f.entry(f.user, "default", "old", "answer").await)
            .await;
        let pending_vector = vector(&p, "old");
        let mut same_dimension = p.clone();
        same_dimension.model = "different weights".into();
        f.set_profile(&same_dimension).await;
        assert!(
            f.cache
                .fill_embedding(&active, &p, &pending_vector)
                .await
                .is_err()
        );
        let fresh = f
            .activate(f.entry(f.user, "default", "fresh", "fresh answer").await)
            .await;
        assert_eq!(
            f.cache
                .fill_embedding(&fresh, &same_dimension, &vector(&same_dimension, "fresh"))
                .await
                .unwrap(),
            CacheCas::Applied(())
        );
        assert!(
            f.cache
                .get_semantic_candidates(f.user, "default", &p, &vector(&p, "q"), 3)
                .await
                .is_err()
        );
        for dimensions in [1024, 3072] {
            let next = profile(dimensions);
            f.set_profile(&next).await;
            let key = format!("dimension {dimensions}");
            let current = f
                .activate(f.entry(f.user, "default", &key, &key).await)
                .await;
            assert_eq!(
                f.cache
                    .fill_embedding(&current, &next, &vector(&next, &key))
                    .await
                    .unwrap(),
                CacheCas::Applied(())
            );
            let results = f
                .cache
                .get_semantic_candidates(f.user, "default", &next, &vector(&next, "query"), 10)
                .await
                .unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].cached.query_text, key);
        }
        f.set_profile(&p).await;
        assert_eq!(
            f.cache
                .fill_embedding(&active, &p, &pending_vector)
                .await
                .unwrap(),
            CacheCas::Stale
        );
    })
    .await;
}

#[tokio::test]
async fn malformed_vectors_and_immutable_profile_descriptors_are_rejected() {
    with_db(|f| async move {
        let p = profile(1536);
        let active = f
            .activate(f.entry(f.user, "default", "input", "answer").await)
            .await;
        for values in [
            vec![0.0; 1536],
            vec![f32::NAN; 1536],
            vec![f32::INFINITY; 1536],
            vec![1.0; 1024],
        ] {
            assert!(
                f.cache
                    .fill_embedding(
                        &active,
                        &p,
                        &EmbeddingVector {
                            input_digest: embedding_text_digest("input"),
                            values
                        }
                    )
                    .await
                    .is_err()
            );
        }
        assert!(
            f.cache
                .fill_embedding(&active, &p, &vector(&p, "different input"))
                .await
                .is_err()
        );
        assert_eq!(f.count("llm_cache_embeddings").await, 0);
        f.cache
            .fill_embedding(&active, &p, &vector(&p, "input"))
            .await
            .unwrap();
        assert!(
            f.db.execute(sql(
                "UPDATE embedding_profiles SET model='silently replaced' WHERE id=$1",
                vec![p.id().unwrap().into()]
            ))
            .await
            .is_err()
        );
        assert!(
            f.db.execute(sql(
                "UPDATE embedding_profiles SET request_descriptor='{}' WHERE id=$1",
                vec![p.id().unwrap().into()]
            ))
            .await
            .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn clear_blocks_late_jobs_and_physically_erases_text_and_vectors_atomically() {
    with_db(|f|async move{
        let p=profile(1536);
        let old_entry=f.entry(f.user,"default","private","private answer").await;
        let active=f.activate(old_entry.clone()).await;
        f.cache.fill_embedding(&active,&p,&vector(&p,"private")).await.unwrap();
        f.db.execute_unprepared("CREATE FUNCTION deny_cache_erase() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected erase failure'; END $$; CREATE TRIGGER deny_cache_erase BEFORE DELETE ON llm_cache FOR EACH ROW EXECUTE FUNCTION deny_cache_erase()").await.unwrap();
        let tx=f.db.begin().await.unwrap();
        tx.execute(sql("UPDATE memory_privacy_state SET privacy_epoch=privacy_epoch+1 WHERE user_id=$1",vec![f.user.into()])).await.unwrap();
        assert!(DbCache::purge_owner_in_transaction(&tx,f.user).await.is_err());tx.rollback().await.unwrap();
        assert!(f.cache.get(f.user,"default","private").await.unwrap().is_some());
        assert_eq!(f.count("llm_cache_embeddings").await,1);
        f.db.execute_unprepared("DROP TRIGGER deny_cache_erase ON llm_cache").await.unwrap();
        let tx=f.db.begin().await.unwrap();
        tx.execute(sql("UPDATE memory_privacy_state SET privacy_epoch=privacy_epoch+1 WHERE user_id=$1",vec![f.user.into()])).await.unwrap();
        assert_eq!(DbCache::purge_owner_in_transaction(&tx,f.user).await.unwrap(),1);tx.commit().await.unwrap();
        assert_eq!(f.count("llm_cache").await,0);assert_eq!(f.count("llm_cache_embeddings").await,0);
        assert!(f.cache.put(old_entry).await.is_err());
        assert_eq!(f.cache.fill_embedding(&active,&p,&vector(&p,"private")).await.unwrap(),CacheCas::Stale);
        assert!(!f.cache.authorize_work(&active,CacheWork::Embedding).await.unwrap());
        assert_eq!(f.count("chat_messages").await,2);
        let new=f.activate(f.entry(f.user,"default","private","new user source").await).await;
        assert_ne!(active.version.cache_id,new.version.cache_id);
    }).await;
}

#[tokio::test]
async fn missing_context_table_fails_without_resolving_any_public_application_table() {
    with_db(|f| async move {
        f.activate(f.entry(f.user, "default", "query", "answer").await)
            .await;
        let mut options = ConnectOptions::new(f.url.clone());
        // pgvector belongs in public, but this exact-read fault test does not
        // need it. Removing public here prevents accidental live-table fallback.
        options
            .max_connections(1)
            .sqlx_logging(false)
            .set_schema_search_path(format!("{},pg_catalog", f.schema));
        let isolated = Database::connect(options).await.unwrap();
        f.db.execute_unprepared("ALTER TABLE context_leases RENAME TO held_context_leases")
            .await
            .unwrap();
        let fault = DbCache::with_database(isolated.clone());
        assert!(fault.get(f.user, "default", "query").await.is_err());
        f.db.execute_unprepared("ALTER TABLE held_context_leases RENAME TO context_leases")
            .await
            .unwrap();
        assert!(
            fault
                .get(f.user, "default", "query")
                .await
                .unwrap()
                .is_some()
        );
        isolated.close().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn cache_source_merge_rechecks_revision_expiry_and_prevents_self_replacement() {
    with_db(|f|async move{
        let active=f.activate(f.entry(f.user,"default","same query","answer").await).await;
        let next=f.generation(f.user,"same query").await;
        let merged=f.context.merge_cache(&next,active.context_identity()).await.unwrap();
        assert!(merged.lease().dependencies().iter().any(|item|
            matches!(item,alia_server::modules::memory::context::ContextEvidence::Cache{id,..} if *id==active.version.cache_id)));
        let derived=f.entry_from(&merged,"default","same query","derived answer").await;
        assert!(f.cache.put(derived).await.is_err());
        assert_eq!(f.cache.get(f.user,"default","same query").await.unwrap().unwrap().version,active.version);
        f.db.execute(sql("UPDATE llm_cache SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1",
            vec![active.version.cache_id.into()])).await.unwrap();
        assert!(f.cache.get(f.user,"default","same query").await.unwrap().is_none());
        let tx=f.db.begin().await.unwrap();
        assert!(ContextService::validate_in_transaction(&tx,&merged.lease().binding(),ContextUse::ResponseCommit).await.is_err());
        tx.rollback().await.unwrap();
        assert_eq!(f.cache.fill_embedding(&active,&profile(1536),&vector(&profile(1536),"same query")).await.unwrap(),CacheCas::Stale);
        assert_eq!(f.cache.cleanup_expired(1).await.unwrap(),1);
    }).await;
}

#[tokio::test]
async fn expiry_cleanup_skips_in_flight_refresh_and_preserves_its_new_revision() {
    with_db(|f| async move {
        let p = profile(1536);
        let active = f
            .activate(f.entry(f.user, "default", "refresh query", "answer").await)
            .await;
        f.cache
            .fill_embedding(&active, &p, &vector(&p, "refresh query"))
            .await
            .unwrap();
        f.db.execute(sql(
            "UPDATE llm_cache SET expires_at=clock_timestamp()-interval '1 second' WHERE id=$1",
            vec![active.version.cache_id.into()],
        )).await.unwrap();

        // This transaction is the database barrier: the committed version is
        // expired, while its replacement has a future TTL and still owns the
        // same row lock used by put. Cleanup sees the old committed snapshot.
        let refresh = f.db.begin().await.unwrap();
        refresh.execute(sql(
            "UPDATE llm_cache SET revision=revision+1,expires_at=clock_timestamp()+interval '1 hour' WHERE id=$1",
            vec![active.version.cache_id.into()],
        )).await.unwrap();
        refresh.execute(sql(
            "UPDATE llm_cache_embeddings SET cache_revision=cache_revision+1 WHERE cache_id=$1",
            vec![active.version.cache_id.into()],
        )).await.unwrap();
        let cache = f.cache.clone();
        let mut cleanup = tokio::spawn(async move { cache.cleanup_expired(1).await });
        let before_commit = tokio::time::timeout(
            std::time::Duration::from_millis(750),
            &mut cleanup,
        )
        .await;
        let skipped_without_waiting = before_commit.is_ok();
        // Always release the writer, including on the old blocking DELETE,
        // so the test observes whether cleanup destroys the refreshed row.
        refresh.commit().await.unwrap();
        let removed = match before_commit {
            Ok(result) => result,
            Err(_) => cleanup.await,
        }
        .unwrap()
        .unwrap();
        let retained = f
            .cache
            .get(f.user, "default", "refresh query")
            .await
            .unwrap()
            .expect("old expiry cleanup deleted the newly refreshed cache version");
        assert_eq!(retained.version.cache_id, active.version.cache_id);
        assert_eq!(retained.version.revision, active.version.revision + 1);
        assert!(retained.expires_at > chrono::Utc::now());
        assert_eq!(f.count("llm_cache_embeddings").await, 1);
        assert_eq!(removed, 0);
        assert!(skipped_without_waiting, "cleanup waited for an active writer");
    })
    .await;
}

#[tokio::test]
async fn expiry_cleanup_caps_each_batch_at_256_and_keeps_live_entries() {
    with_db(|f| async move {
        let entry = f.entry(f.user, "live", "batch query", "answer").await;
        f.cache.put(entry.clone()).await.unwrap();
        for index in 0..257 {
            let mut expired = entry.clone();
            expired.scope = format!("expired-{index}");
            f.cache.put(expired).await.unwrap();
        }
        f.db.execute(sql(
            "UPDATE llm_cache SET expires_at=clock_timestamp()-interval '1 second' WHERE scope<>'live'",
            vec![],
        )).await.unwrap();
        assert_eq!(f.cache.cleanup_expired(usize::MAX).await.unwrap(), 256);
        assert_eq!(f.count("llm_cache").await, 2);
        assert_eq!(f.cache.cleanup_expired(usize::MAX).await.unwrap(), 1);
        assert_eq!(f.cache.cleanup_expired(0).await.unwrap(), 0);
        assert_eq!(f.count("llm_cache").await, 1);
        assert_eq!(
            f.db.query_one(sql("SELECT scope FROM llm_cache", vec![]))
                .await.unwrap().unwrap().try_get::<String>("", "scope").unwrap(),
            "live"
        );
    })
    .await;
}

#[tokio::test]
async fn replacement_rollback_keeps_previous_reviewed_content_and_vector() {
    with_db(|f|async move{
        let p=profile(1536);
        let active=f.activate(f.entry(f.user,"default","query","old answer").await).await;
        f.cache.fill_embedding(&active,&p,&vector(&p,"query")).await.unwrap();
        f.db.execute_unprepared("CREATE FUNCTION deny_vector_removal() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected cache replacement failure'; END $$; CREATE TRIGGER deny_vector_removal BEFORE DELETE ON llm_cache_embeddings FOR EACH ROW EXECUTE FUNCTION deny_vector_removal()").await.unwrap();
        assert!(f.cache.put(f.entry(f.user,"default","query","replacement").await).await.is_err());
        let unchanged=f.cache.get(f.user,"default","query").await.unwrap().unwrap();
        assert_eq!(unchanged.version,active.version);assert_eq!(unchanged.response_text,"old answer");
        assert_eq!(f.count("llm_cache_embeddings").await,1);
    }).await;
}

#[cfg(feature = "insights")]
mod insights {
    use super::*;
    use alia_server::{
        config::{MemoryCaptureMode, ServerConfig},
        event::EventBus,
        module::{ModuleLifecycle, lifecycle::ModuleContext},
        plugins::PluginStorageService,
        providers::{insights::ScheduledInsights, memory::AliaMemory, sensor::SensorRegistry},
        registry::ArtifactRegistry,
        settings::SettingsRegistry,
    };
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;

    struct HeldReviewer {
        hold_once: AtomicBool,
        entered: Notify,
        release: Notify,
        reply: String,
        calls: AtomicUsize,
        messages: tokio::sync::Mutex<Vec<ChatMessage>>,
    }
    #[async_trait]
    impl ModuleLifecycle for HeldReviewer {
        fn name(&self) -> &'static str {
            "held-cache-reviewer"
        }
    }
    #[async_trait]
    impl LlmAdapter for HeldReviewer {
        async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.messages.lock().await = messages.to_vec();
            if self.hold_once.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(self.reply.clone())
        }
        async fn chat_stream(
            &self,
            _messages: &[ChatMessage],
        ) -> anyhow::Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<String>> + Send>>,
        > {
            anyhow::bail!("insights tests use nonstreaming calls")
        }
    }
    fn probe(reply: Value, hold: bool) -> Arc<HeldReviewer> {
        Arc::new(HeldReviewer {
            hold_once: AtomicBool::new(hold),
            entered: Notify::new(),
            release: Notify::new(),
            reply: reply.to_string(),
            calls: AtomicUsize::new(0),
            messages: tokio::sync::Mutex::new(Vec::new()),
        })
    }

    struct CountingEmbedding(AtomicUsize);
    #[async_trait]
    impl ModuleLifecycle for CountingEmbedding {
        fn name(&self) -> &'static str {
            "counting-profiled-embedding"
        }
    }
    #[async_trait]
    impl EmbeddingAdapter for CountingEmbedding {
        async fn embed(&self, _: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
            anyhow::bail!("cache must use the profiled request")
        }
        async fn embed_profiled(
            &self,
            texts: &[String],
        ) -> anyhow::Result<Option<ProfiledEmbeddings>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let p = profile(1536);
            Ok(Some(ProfiledEmbeddings {
                vectors: texts.iter().map(|text| vector(&p, text)).collect(),
                profile: p,
            }))
        }
        fn dimension(&self) -> usize {
            1536
        }
    }
    async fn reviewer(
        f: &Fixture,
        llm: Arc<HeldReviewer>,
        embedding: Arc<CountingEmbedding>,
        memory: Option<Arc<dyn MemoryBackend>>,
    ) -> Arc<ScheduledInsights> {
        let mut config = ServerConfig::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../config/server.example.toml"),
        )
        .unwrap();
        config.policies.insights.enabled = true;
        let context = ModuleContext {
            db: f.db.clone(),
            config: Arc::new(config),
            event_bus: EventBus::new(32),
            cancel_token: CancellationToken::new(),
            settings: SettingsRegistry::new(),
            artifacts: Arc::new(ArtifactRegistry::new(f.db.clone())),
            plugin_storage: Arc::new(PluginStorageService::new(f.db.clone())),
            sensors: Arc::new(SensorRegistry::new(Default::default())),
            scheduler: None,
            conditions: None,
            actions: None,
            conversation: None,
            memory,
            llm: Some(llm),
            minor_llm: None,
            major_llm: None,
            memory_capture_llm: None,
            sentence_expression_llm: None,
            embedding: Some(embedding),
            cache: Some(f.cache.clone()),
            tools: None,
            expression: None,
        };
        let mut service = ScheduledInsights::new();
        service.init(&context).await.unwrap();
        Arc::new(service)
    }

    #[tokio::test]
    async fn held_review_cannot_activate_replacement_or_count_a_stale_decision() {
        with_db(|f|async move{
            let old=f.cache.put(f.entry(f.user,"default","same","old answer").await).await.unwrap();
            let llm=probe(json!({"decision":"approve","refined_response":"reviewed answer",
                "safe_direct_return":true,"memory_independent":true,"tool_free":true,"dynamic_free":true}),true);
            let embedding=Arc::new(CountingEmbedding(AtomicUsize::new(0)));
            let service=reviewer(&f,llm.clone(),embedding.clone(),None).await;
            let job=service.clone();let owner=f.user;
            let task=tokio::spawn(async move{job.trigger(owner,InsightPhase::Consolidation).await});
            tokio::time::timeout(std::time::Duration::from_secs(2),llm.entered.notified()).await.unwrap();
            assert_eq!(embedding.0.load(Ordering::SeqCst),0);
            let replacement=f.cache.put(f.entry(f.user,"default","same","replacement answer").await).await.unwrap();
            assert!(replacement.version.revision>old.version.revision);
            llm.release.notify_one();task.await.unwrap().unwrap();
            let pending=f.cache.load_pending(f.user,None,10).await.unwrap();
            assert_eq!(pending.len(),1);assert_eq!(pending[0].version,replacement.version);
            assert_eq!(pending[0].response_text,"replacement answer");
            assert_eq!(embedding.0.load(Ordering::SeqCst),0);
            let log=f.db.query_one(sql("SELECT nodes_updated,summary FROM insight_logs ORDER BY created_at DESC LIMIT 1",vec![]))
                .await.unwrap().unwrap();
            assert_eq!(log.try_get::<i32>("","nodes_updated").unwrap(),0);
            assert!(log.try_get::<String>("","summary").unwrap().contains("approved 0, deleted 0"));
            service.trigger(f.user,InsightPhase::Consolidation).await.unwrap();
            let active=f.cache.get(f.user,"default","same").await.unwrap().unwrap();
            assert_eq!(active.response_text,"reviewed answer");
            assert_eq!(active.version.revision,replacement.version.revision+1);
            assert_eq!(embedding.0.load(Ordering::SeqCst),1);
            assert_eq!(f.count("llm_cache_embeddings").await,1);
            service.stop().await.unwrap();
        }).await;
    }

    async fn memory(f: &Fixture) -> Arc<AliaMemory> {
        let memory = Arc::new(AliaMemory::new());
        memory
            .attach_runtime_for_test(f.db.clone(), None, MemorySection::default())
            .await
            .unwrap();
        memory
    }

    async fn capture_policy(f: &Fixture, mode: MemoryCaptureMode) {
        let current = f.context.policy_store().current().await.unwrap();
        let mut policy = current.policy;
        policy.memory_enabled = true;
        policy.capture_mode = mode;
        policy.consolidation_enabled = true;
        f.context
            .policy_store()
            .replace(current.version, policy)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn automatic_insight_phases_obey_capture_policy_and_use_only_canonical_user_rows() {
        with_db(|f|async move{
            let memory=memory(&f).await;
            let llm=probe(json!({"memories":[{"content":"The user likes jasmine tea","confidence":0.99,"importance":0.8}]}),false);
            let service=reviewer(&f,llm.clone(),Arc::new(CountingEmbedding(AtomicUsize::new(0))),Some(memory)).await;
            let source = format!("I like jasmine tea {} CANONICAL_SOURCE_TAIL", "x".repeat(10_000));
            f.entry(f.user,"default",&source,"ASSISTANT_ONLY_DO_NOT_CAPTURE").await;
            for mode in [MemoryCaptureMode::None,MemoryCaptureMode::ExplicitToolOnly] {
                capture_policy(&f,mode).await;
                for phase in [InsightPhase::Extraction,InsightPhase::Reflection,InsightPhase::HabitDetection] {
                    service.trigger(f.user,phase).await.unwrap();
                }
            }
            assert_eq!(llm.calls.load(Ordering::SeqCst),0);
            assert_eq!(f.count("memory_items").await,0);
            capture_policy(&f,MemoryCaptureMode::LlmSelected).await;
            service.trigger(f.user,InsightPhase::Extraction).await.unwrap();
            assert_eq!(llm.calls.load(Ordering::SeqCst),1);
            let messages=llm.messages.lock().await;
            let prompt=messages.iter().map(|message|message.content.as_str()).collect::<Vec<_>>().join("\n");
            assert!(prompt.contains("I like jasmine tea"));
            assert!(prompt.contains("CANONICAL_SOURCE_TAIL"));
            assert!(!prompt.contains("ASSISTANT_ONLY_DO_NOT_CAPTURE"));
            drop(messages);
            assert_eq!(f.count("memory_items").await,1);
            let row=f.db.query_one(sql("SELECT count(*) AS n FROM context_leases WHERE purpose='memory_capture'",vec![]))
                .await.unwrap().unwrap();
            assert_eq!(row.try_get::<i64>("","n").unwrap(),1);
            service.stop().await.unwrap();
        }).await;
    }

    #[tokio::test]
    async fn clear_while_automatic_extractor_waits_blocks_its_old_model_result() {
        with_db(|f|async move{
            let memory=memory(&f).await;
            capture_policy(&f,MemoryCaptureMode::LlmSelected).await;
            f.generation(f.user,"A current user preference").await;
            let llm=probe(json!({"memories":[{"content":"A derived preference","confidence":0.99,"importance":0.8}]}),true);
            let service=reviewer(&f,llm.clone(),Arc::new(CountingEmbedding(AtomicUsize::new(0))),Some(memory.clone())).await;
            let worker=service.clone();let owner=f.user;
            let task=tokio::spawn(async move{worker.trigger(owner,InsightPhase::Extraction).await});
            tokio::time::timeout(std::time::Duration::from_secs(2),llm.entered.notified()).await.unwrap();
            memory.clear(f.user).await.unwrap();
            llm.release.notify_one();
            assert!(task.await.unwrap().is_err());
            assert_eq!(f.count("memory_items").await,0);
            assert_eq!(f.count("chat_messages").await,1);
            service.stop().await.unwrap();
        }).await;
    }

    #[tokio::test]
    async fn consolidation_uses_maintenance_lease_and_persistent_enable_switch() {
        with_db(|f|async move{
            let memory=memory(&f).await;
            capture_policy(&f,MemoryCaptureMode::None).await;
            let id=memory.store(f.user,MemoryNode::new("core","fact","Original memory",0.8)).await.unwrap();
            let llm=probe(json!({"updates":[{"id":id,"content":"Consolidated memory"}],"retirements":[],"creates":[]}),false);
            let service=reviewer(&f,llm.clone(),Arc::new(CountingEmbedding(AtomicUsize::new(0))),Some(memory.clone())).await;
            let current=f.context.policy_store().current().await.unwrap();
            let mut policy=current.policy;policy.consolidation_enabled=false;
            f.context.policy_store().replace(current.version,policy).await.unwrap();
            service.trigger(f.user,InsightPhase::Consolidation).await.unwrap();
            assert_eq!(llm.calls.load(Ordering::SeqCst),0);
            assert_eq!(memory.get(f.user,&id).await.unwrap().unwrap().content,"Original memory");
            let current=f.context.policy_store().current().await.unwrap();
            let mut policy=current.policy;policy.consolidation_enabled=true;
            f.context.policy_store().replace(current.version,policy).await.unwrap();
            service.trigger(f.user,InsightPhase::Consolidation).await.unwrap();
            assert_eq!(llm.calls.load(Ordering::SeqCst),1);
            assert_eq!(memory.get(f.user,&id).await.unwrap().unwrap().content,"Consolidated memory");
            let row=f.db.query_one(sql("SELECT count(*) AS n FROM context_leases WHERE purpose='memory_maintenance'",vec![]))
                .await.unwrap().unwrap();
            assert_eq!(row.try_get::<i64>("","n").unwrap(),1);
            service.stop().await.unwrap();
        }).await;
    }
}
