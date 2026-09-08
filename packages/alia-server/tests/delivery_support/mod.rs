#![allow(dead_code)]
//! These tests require real disposable PostgreSQL; missing infrastructure fails.
use alia_core::delivery::{PlaybackIdentity, PlaybackProgress, VoiceScope};
use alia_server::modules::{
    delivery::{repository::DeliveryRepository, types::*},
    platform::migrations,
};
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::json;
use std::{future::Future, panic::AssertUnwindSafe, time::Duration};
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
    pub fn response(&self) -> NewResponse {
        NewResponse {
            key: ResponseKey {
                user_id: self.user,
                request_id: Uuid::new_v4(),
                output_key: "answer".into(),
            },
            conversation_id: None,
            source_event_id: None,
            source_received_at: chrono::Utc::now(),
            source_privacy_epoch: Some(0),
            channel_id: "test-device".into(),
            kind: "conversation".into(),
            content: json!({"intent":"answer source"}),
        }
    }
    pub async fn prepared(&self) -> (ResponseRecord, NewDelivery, DeliveryRecord) {
        let response = self.repo().prepare(self.response()).await.unwrap();
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
