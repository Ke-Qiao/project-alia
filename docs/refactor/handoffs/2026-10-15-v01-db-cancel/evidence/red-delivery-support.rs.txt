#![allow(dead_code)]
//! These tests require real disposable PostgreSQL; missing infrastructure fails.
use alia_core::delivery::{PlaybackIdentity, PlaybackProgress, VoiceScope};
use alia_server::modules::{
    delivery::{repository::DeliveryRepository, types::*},
    memory::context::{ContextPolicy, ContextService, ContextSource},
    platform::migrations,
};
use futures::FutureExt;
use futures::future::LocalBoxFuture;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::json;
use std::{future::Future, panic::AssertUnwindSafe, sync::Arc, time::Duration};
use uuid::Uuid;
static CASES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);
const CASE_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone)]
pub struct Fixture {
    admin: DatabaseConnection,
    pub db: DatabaseConnection,
    schema: String,
    pub user: Uuid,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum InitializationStep {
    WorkPool,
    UserSeed,
    Policy,
}

impl Fixture {
    async fn new() -> Self {
        Self::new_with(|_, _| Box::pin(async {})).await
    }
    async fn new_with<H>(mut checkpoint: H) -> Self
    where
        H: for<'a> FnMut(InitializationStep, &'a Self) -> LocalBoxFuture<'a, ()>,
    {
        let url = std::env::var("ALIA_TEST_DATABASE_URL")
            .expect("set ALIA_TEST_DATABASE_URL to disposable PostgreSQL");
        let mut options = ConnectOptions::new(url.clone());
        options.max_connections(1).sqlx_logging(false);
        let admin = Database::connect(options)
            .await
            .expect("PostgreSQL must be reachable");
        let mut f = Self {
            admin,
            db: DatabaseConnection::Disconnected,
            schema: format!("delivery_test_{}", Uuid::new_v4().simple()),
            user: Uuid::new_v4(),
        };
        let initialized = AssertUnwindSafe(async {
            f.admin
                .execute_unprepared(&format!("CREATE SCHEMA {}", f.schema))
                .await
                .unwrap();
            checkpoint(InitializationStep::WorkPool, &f).await;
            let mut options = ConnectOptions::new(url);
            options
                .max_connections(4)
                .set_schema_search_path(format!("{},public", f.schema))
                .sqlx_logging(false);
            f.db = Database::connect(options).await.unwrap();
            if let Err(error) = migrations::run(&f.db).await {
                panic!("install migrations: {error:#}");
            }
            checkpoint(InitializationStep::UserSeed, &f).await;
            f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Delivery test','not-a-login','user','active')",vec![f.user.into(),format!("{}@delivery.test", f.user).into()])).await.unwrap();
            checkpoint(InitializationStep::Policy, &f).await;
            f.context()
                .initialize_policy(
                    ContextPolicy::from_config(
                        &alia_server::config::MemorySection::default(),
                        &alia_server::config::ConversationSection::default(),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            alia_server::modules::memory::source::capture_source_epoch(&f.db, f.user)
                .await
                .unwrap();
        })
        .catch_unwind()
        .await;
        if let Err(error) = initialized {
            if AssertUnwindSafe(f.cleanup()).catch_unwind().await.is_err() {
                eprintln!("Delivery initialization cleanup failed");
            }
            std::panic::resume_unwind(error);
        }
        f
    }
    pub(crate) fn schema_name(&self) -> &str {
        &self.schema
    }
    async fn cleanup(self) {
        let work_pool = if matches!(self.db, DatabaseConnection::Disconnected) {
            Ok(())
        } else {
            self.db.close().await
        };
        let schema = self
            .admin
            .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.schema))
            .await;
        let admin_pool = self.admin.close().await;
        assert!(
            work_pool.is_ok() && schema.is_ok() && admin_pool.is_ok(),
            "Delivery database cleanup failed: work_pool={}, schema={}, admin_pool={}",
            work_pool.is_ok(),
            schema.is_ok(),
            admin_pool.is_ok()
        );
    }
    pub fn repo(&self) -> DeliveryRepository {
        DeliveryRepository::new(self.db.clone())
    }
    pub fn context(&self) -> ContextService {
        ContextService::new(
            self.db.clone(),
            Arc::new(alia_server::providers::embedding::NoopEmbedding),
        )
    }
    pub async fn response(&self) -> NewResponse {
        self.response_in_session(Uuid::new_v4()).await
    }
    pub async fn notice(&self) -> NewResponse {
        let mut input = self.response().await;
        let lease = self
            .context()
            .source_notice(ContextSource {
                owner_id: self.user,
                request_id: input.key.request_id,
                channel_id: input.channel_id.clone(),
                conversation_id: input.conversation_id,
                source_event_id: input.source_event_id,
                source_received_at: input.source_received_at,
                source_privacy_epoch: input.source_privacy_epoch.unwrap(),
            })
            .await
            .unwrap();
        input.context_id = lease.id();
        input.kind = "source_notice".into();
        input.content = json!({"text": lease.notice_text().unwrap()});
        input
    }
    pub async fn response_in_session(&self, session: Uuid) -> NewResponse {
        self.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"INSERT INTO chat_sessions(id,channel_id,creator_id) VALUES($1,'test-device',$2) ON CONFLICT(id) DO NOTHING",vec![session.into(),self.user.to_string().into()])).await.unwrap();
        let epoch = alia_server::modules::memory::source::capture_source_epoch(&self.db, self.user)
            .await
            .unwrap();
        let event = Uuid::new_v4();
        let request = Uuid::new_v4();
        let row=self.db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,"INSERT INTO chat_messages(id,session_id,user_id,role,content,source_received_at,source_privacy_epoch) VALUES($1,$2,$3,'user','answer source',clock_timestamp(),$4) RETURNING source_received_at",vec![event.into(),session.into(),self.user.to_string().into(),epoch.into()])).await.unwrap().unwrap();
        let source = ContextSource {
            owner_id: self.user,
            request_id: request,
            channel_id: "test-device".into(),
            conversation_id: Some(session),
            source_event_id: Some(event),
            source_received_at: row.try_get("", "source_received_at").unwrap(),
            source_privacy_epoch: epoch,
        };
        let context = self
            .context()
            .materialize(source.clone(), "answer source")
            .await
            .unwrap();
        NewResponse {
            key: ResponseKey {
                user_id: self.user,
                request_id: request,
                output_key: "answer".into(),
            },
            context_id: context.lease().id(),
            conversation_id: Some(session),
            source_event_id: Some(event),
            source_received_at: source.source_received_at,
            source_privacy_epoch: Some(epoch),
            channel_id: "test-device".into(),
            kind: "conversation".into(),
            content: json!({"intent":"answer source"}),
        }
    }
    pub async fn prepared(&self) -> (ResponseRecord, NewDelivery, DeliveryRecord) {
        let response = self.repo().prepare(self.response().await).await.unwrap();
        let input = NewDelivery {
            response_id: response.response_id,
            output_key: "speech_sentence:0".into(),
            content: json!({"text":"hello"}),
            identity: Some(identity()),
            owner_id: Uuid::new_v4(),
        };
        let delivery = self
            .repo()
            .prepare_delivery(self.user, input.clone())
            .await
            .unwrap();
        (response, input, delivery)
    }
    pub async fn sql(&self, sql: &str) {
        self.db.execute_unprepared(sql).await.unwrap();
    }
    pub async fn count(&self, table: &str) -> i64 {
        assert!(table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        self.db
            .query_one(Statement::from_string(
                DatabaseBackend::Postgres,
                format!("SELECT COUNT(*) AS n FROM {table}"),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap()
    }
    pub async fn assistant_count(&self) -> i64 {
        self.db
            .query_one(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT COUNT(*) AS n FROM chat_messages WHERE role='assistant'",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap()
    }
}
pub fn identity() -> PlaybackIdentity {
    PlaybackIdentity {
        scope: VoiceScope {
            device_boot_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            generation_epoch: 1,
        },
        playback_id: Uuid::new_v4(),
        playback_sequence: 1,
    }
}
pub fn receipt(identity: PlaybackIdentity, seq: u64, accepted: u64) -> PlaybackProgress {
    PlaybackProgress {
        identity,
        receipt_sequence: seq,
        accepted_source_frames: accepted,
        submitted_source_frames: None,
        presented_source_frames: None,
        end_accepted: false,
        backend: None,
        observation_end: None,
    }
}
pub(crate) async fn probe_initialization<H>(checkpoint: H)
where
    H: for<'a> FnMut(InitializationStep, &'a Fixture) -> LocalBoxFuture<'a, ()>,
{
    let _slot = CASES.acquire().await.unwrap();
    let f = Fixture::new_with(checkpoint).await;
    f.cleanup().await;
}

pub async fn with_db<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    let _slot = CASES.acquire().await.unwrap();
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(tokio::time::timeout(CASE_TIMEOUT, async {
        test(f.clone()).await
    }))
    .catch_unwind()
    .await;
    finish_case(result, None, || async {}, || f.cleanup()).await;
}

/// Keep resource owners outside the timed body so cancellation can still join
/// them before the database closes. Body and context cleanup share the same 45s.
pub async fn with_db_context<C, F, Cleanup, CleanupFuture>(context: C, test: F, cleanup: Cleanup)
where
    F: for<'a> FnOnce(Fixture, &'a mut C) -> LocalBoxFuture<'a, ()>,
    Cleanup: FnOnce(C) -> CleanupFuture,
    CleanupFuture: Future<Output = ()>,
{
    let _slot = CASES.acquire().await.unwrap();
    let f = Fixture::new().await;
    let body_fixture = f.clone();
    run_context(
        context,
        move |context| test(body_fixture, context),
        cleanup,
        || f.cleanup(),
    )
    .await;
}

pub(crate) async fn run_context<C, F, Cleanup, CleanupFuture, DatabaseCleanup, DatabaseFuture>(
    mut context: C,
    test: F,
    cleanup: Cleanup,
    database_cleanup: DatabaseCleanup,
) where
    F: for<'a> FnOnce(&'a mut C) -> LocalBoxFuture<'a, ()>,
    Cleanup: FnOnce(C) -> CleanupFuture,
    CleanupFuture: Future<Output = ()>,
    DatabaseCleanup: FnOnce() -> DatabaseFuture,
    DatabaseFuture: Future<Output = ()>,
{
    let deadline = tokio::time::Instant::now() + CASE_TIMEOUT;
    let result = AssertUnwindSafe(tokio::time::timeout_at(deadline, async {
        test(&mut context).await
    }))
    .catch_unwind()
    .await;
    finish_case(
        result,
        Some(deadline),
        || cleanup(context),
        database_cleanup,
    )
    .await;
}

async fn finish_case<Cleanup, CleanupFuture, DatabaseCleanup, DatabaseFuture>(
    result: std::thread::Result<Result<(), tokio::time::error::Elapsed>>,
    deadline: Option<tokio::time::Instant>,
    cleanup: Cleanup,
    database_cleanup: DatabaseCleanup,
) where
    Cleanup: FnOnce() -> CleanupFuture,
    CleanupFuture: Future<Output = ()>,
    DatabaseCleanup: FnOnce() -> DatabaseFuture,
    DatabaseFuture: Future<Output = ()>,
{
    let cleanup = AssertUnwindSafe(async { cleanup().await }).catch_unwind();
    tokio::pin!(cleanup);
    let mut cleanup_expired = false;
    let cleanup = if let Some(deadline) = deadline {
        match tokio::time::timeout_at(deadline, cleanup.as_mut()).await {
            Ok(result) => {
                cleanup_expired = tokio::time::Instant::now() >= deadline;
                result
            }
            Err(_) => {
                cleanup_expired = true;
                // Expiry fails the case; it cannot release a still-owned cleanup.
                cleanup.await
            }
        }
    } else {
        cleanup.await
    };
    let database_cleanup = AssertUnwindSafe(async { database_cleanup().await })
        .catch_unwind()
        .await;
    if cleanup.is_err() {
        eprintln!("Delivery context cleanup failed");
    }
    if database_cleanup.is_err() {
        eprintln!("Delivery database cleanup failed");
    }
    match result {
        Ok(Ok(())) if cleanup_expired => panic!("Delivery test exceeded 45s"),
        Ok(Ok(())) => {
            if let Err(error) = cleanup {
                std::panic::resume_unwind(error);
            }
            if let Err(error) = database_cleanup {
                std::panic::resume_unwind(error);
            }
        }
        Ok(Err(_)) => panic!("Delivery test exceeded 45s"),
        Err(error) => std::panic::resume_unwind(error),
    }
}
