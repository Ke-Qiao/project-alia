use super::*;
use futures::FutureExt;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseBackend, Statement};
use std::panic::AssertUnwindSafe;
use std::time::Duration;

#[tokio::test]
async fn durable_input_provenance_preserves_time_owner_and_atomicity() {
    let url = std::env::var("ALIA_TEST_DATABASE_URL")
        .expect("set ALIA_TEST_DATABASE_URL to disposable PostgreSQL");
    let admin = Database::connect(&url).await.unwrap();
    let schema = format!("input_source_test_{}", Uuid::new_v4().simple());
    admin
        .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
        .await
        .unwrap();
    let mut options = ConnectOptions::new(url);
    options
        .max_connections(2)
        .set_schema_search_path(format!("{schema},public"));
    let db = Database::connect(options).await.unwrap();
    let result = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(30), async {
        crate::modules::platform::migrations::run(&db).await.unwrap();
        let mut backend = DatabaseConversationBackend::new();
        backend.db = Some(db.clone());
        let owner = Uuid::new_v4();
        use crate::modules::platform::identity::require_active_owner;
        assert!(require_active_owner(Some(&db), Some(owner)).await.is_err());
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Source test','not-a-login-hash','user','active')", vec![owner.into(),format!("{owner}@source.test").into()])).await.unwrap();
        assert_eq!(require_active_owner(Some(&db), Some(owner)).await.unwrap(), owner);
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE users SET status='disabled' WHERE id=$1", [owner.into()])).await.unwrap();
        assert!(require_active_owner(Some(&db), Some(owner)).await.is_err());
        let session = backend.create_session("voice-test", &owner.to_string()).await.unwrap();
        let received = chrono::Utc::now() - chrono::Duration::hours(1);
        let mut message = ChatMessage::new("user", "用户喜欢咖啡");
        message.name = Some(Uuid::new_v4().to_string());
        let id = backend.append_sourced_message(owner, session, message, received).await.unwrap();
        let row = db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT user_id,source_received_at,created_at FROM chat_messages WHERE id=$1", [id.into()])).await.unwrap().unwrap();
        assert_eq!(row.try_get::<String>("", "user_id").unwrap(), owner.to_string());
        assert_eq!(row.try_get::<chrono::DateTime<chrono::Utc>>("", "source_received_at").unwrap().timestamp_micros(), received.timestamp_micros());
        assert!(row.try_get::<chrono::DateTime<chrono::Utc>>("", "created_at").unwrap() > received + chrono::Duration::minutes(59));
        assert!(backend.append_sourced_message(Uuid::new_v4(), session, ChatMessage::new("user", "forged"), received).await.is_err());
        assert!(backend.append_sourced_message(owner, session, ChatMessage::new("assistant", "not user evidence"), received).await.is_err());

        // The source insert must roll back if updating its conversation fails.
        db.execute_unprepared("CREATE FUNCTION fail_source_session_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected session update failure'; END $$; CREATE TRIGGER fail_source_session_update BEFORE UPDATE ON chat_sessions FOR EACH ROW EXECUTE FUNCTION fail_source_session_update()").await.unwrap();
        assert!(backend.append_sourced_message(owner, session, ChatMessage::new("user", "must roll back"), received).await.is_err());
        assert!(backend.append_message(session, ChatMessage::new("assistant", "reply must also roll back")).await.is_err());
        let count: i64 = db.query_one(Statement::from_string(DatabaseBackend::Postgres, "SELECT COUNT(*) AS count FROM chat_messages")).await.unwrap().unwrap().try_get("", "count").unwrap();
        assert_eq!(count, 1);
    })).catch_unwind().await;
    db.close().await.unwrap();
    admin
        .execute_unprepared(&format!("DROP SCHEMA {schema} CASCADE"))
        .await
        .unwrap();
    admin.close().await.unwrap();
    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("provenance test exceeded deadline"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
