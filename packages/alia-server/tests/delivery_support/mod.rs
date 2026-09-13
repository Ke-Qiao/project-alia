#![allow(dead_code)]
//! These tests require real disposable PostgreSQL; missing infrastructure fails.
use alia_core::delivery::{PlaybackIdentity, PlaybackProgress, VoiceScope};
use alia_server::modules::{
    delivery::{repository::DeliveryRepository, types::*},
    memory::context::{ContextPolicy, ContextService, ContextSource},
    platform::migrations,
};
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::json;
use std::{future::Future, panic::AssertUnwindSafe, sync::Arc, time::Duration};
use uuid::Uuid;
static CASES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);

#[derive(Clone)]
pub struct Fixture {
    admin: DatabaseConnection,
    pub db: DatabaseConnection,
    schema: String,
    pub user: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("ALIA_TEST_DATABASE_URL")
            .expect("set ALIA_TEST_DATABASE_URL to disposable PostgreSQL");
        let mut options = ConnectOptions::new(url.clone());
        options.max_connections(1).sqlx_logging(false);
        let admin = Database::connect(options)
            .await
            .expect("PostgreSQL must be reachable");
        let schema = format!("delivery_test_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut options = ConnectOptions::new(url);
        options
            .max_connections(4)
            .set_schema_search_path(format!("{schema},public"))
            .sqlx_logging(false);
        let db = Database::connect(options).await.unwrap();
        let user = Uuid::new_v4();
        let f = Self {
            admin,
            db,
            schema,
            user,
        };
        if let Err(error) = migrations::run(&f.db).await {
            f.cleanup().await;
            panic!("install migrations: {error:#}");
        }
        f.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Delivery test','not-a-login','user','active')",vec![user.into(),format!("{user}@delivery.test").into()])).await.unwrap();
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
        alia_server::modules::memory::source::capture_source_epoch(&f.db, user)
            .await
            .unwrap();
        f
    }
    async fn cleanup(self) {
        self.db.close().await.unwrap();
        self.admin
            .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .await
            .unwrap();
        self.admin.close().await.unwrap();
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
pub async fn with_db<F, Fut>(test: F)
where
    F: FnOnce(Fixture) -> Fut,
    Fut: Future<Output = ()>,
{
    let _slot = CASES.acquire().await.unwrap();
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(45),
        test(f.clone()),
    ))
    .catch_unwind()
    .await;
    f.cleanup().await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("Delivery test exceeded 45s"),
        Err(error) => std::panic::resume_unwind(error),
    }
}
