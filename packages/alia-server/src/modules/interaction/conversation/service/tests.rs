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
        let backend = ConversationService::with_database(db.clone());
        let owner = Uuid::new_v4();
        use crate::modules::platform::identity::require_active_owner;
        assert!(require_active_owner(Some(&db), Some(owner)).await.is_err());
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO users(id,email,name,password_hash,role,status) VALUES($1,$2,'Source test','not-a-login-hash','user','active')", vec![owner.into(),format!("{owner}@source.test").into()])).await.unwrap();
        assert_eq!(require_active_owner(Some(&db), Some(owner)).await.unwrap(), owner);
        let admitted = crate::modules::memory::source::capture_source_epochs(&db, owner).await.unwrap();
        let epoch = admitted.privacy_epoch;
        assert_eq!(admitted.relationship_epoch, 0);
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE users SET status='disabled' WHERE id=$1", [owner.into()])).await.unwrap();
        assert!(require_active_owner(Some(&db), Some(owner)).await.is_err());
        let session = backend.create_session("voice-test", &owner.to_string()).await.unwrap();
        let received = chrono::Utc::now() - chrono::Duration::hours(1);
        let mut message = ChatMessage::new("user", "用户喜欢咖啡");
        message.user_id = Some(Uuid::new_v4().to_string());
        let id = backend.append_sourced_message(owner, session, message, received, epoch, admitted.relationship_epoch).await.unwrap();
        let row = db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT user_id,source_received_at,source_privacy_epoch,source_relationship_epoch,created_at FROM chat_messages WHERE id=$1", [id.into()])).await.unwrap().unwrap();
        assert_eq!(row.try_get::<String>("", "user_id").unwrap(), owner.to_string());
        assert_eq!(row.try_get::<i64>("", "source_privacy_epoch").unwrap(), epoch);
        assert_eq!(row.try_get::<i64>("", "source_relationship_epoch").unwrap(), admitted.relationship_epoch);
        assert_eq!(row.try_get::<chrono::DateTime<chrono::Utc>>("", "source_received_at").unwrap().timestamp_micros(), received.timestamp_micros());
        assert!(row.try_get::<chrono::DateTime<chrono::Utc>>("", "created_at").unwrap() > received + chrono::Duration::minutes(59));
        assert!(backend.append_sourced_message(Uuid::new_v4(), session, ChatMessage::new("user", "forged"), received, epoch, admitted.relationship_epoch).await.is_err());
        assert!(backend.append_sourced_message(owner, session, ChatMessage::new("assistant", "not user evidence"), received, epoch, admitted.relationship_epoch).await.is_err());

        // Admission is the causal boundary. An old source still preserves the
        // user's original message if it reaches persistence after clear.
        use crate::modules::memory::MemoryService;
        let memory = MemoryService::new();
        memory.attach_runtime_for_test(db.clone(), None, Default::default()).await.unwrap();
        assert!(memory.clear(owner).await.is_err());
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE users SET status='active' WHERE id=$1", [owner.into()],
        )).await.unwrap();
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE relationship_authority SET relationship_epoch=relationship_epoch+1 WHERE owner_id=$1", [owner.into()],
        )).await.unwrap();
        let relationship_clear = crate::modules::memory::source::capture_source_epochs(&db, owner).await.unwrap();
        assert_eq!(relationship_clear.privacy_epoch, admitted.privacy_epoch);
        assert_eq!(relationship_clear.relationship_epoch, admitted.relationship_epoch + 1);
        let relationship_late = backend.append_sourced_message(owner, session,
            ChatMessage::new("user", "关系清除前已接纳的原始消息"), received,
            admitted.privacy_epoch, admitted.relationship_epoch).await.unwrap();
        let relationship_late_row = db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT source_relationship_epoch FROM chat_messages WHERE id=$1", [relationship_late.into()],
        )).await.unwrap().unwrap();
        assert_eq!(relationship_late_row.try_get::<i64>("", "source_relationship_epoch").unwrap(), admitted.relationship_epoch);
        memory.clear(owner).await.unwrap();
        let late_time = chrono::Utc::now() + chrono::Duration::minutes(5);
        let late = backend.append_sourced_message(owner, session, ChatMessage::new("user", "原始消息仍须保存"), late_time, epoch, admitted.relationship_epoch).await.unwrap();
        let late_row = db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT content,source_privacy_epoch,source_relationship_epoch FROM chat_messages WHERE id=$1", [late.into()],
        )).await.unwrap().unwrap();
        assert_eq!(late_row.try_get::<String>("", "content").unwrap(), "原始消息仍须保存");
        assert_eq!(late_row.try_get::<i64>("", "source_privacy_epoch").unwrap(), epoch);
        assert_eq!(late_row.try_get::<i64>("", "source_relationship_epoch").unwrap(), admitted.relationship_epoch);
        assert!(db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE chat_messages SET source_relationship_epoch=source_relationship_epoch+1 WHERE id=$1", [late.into()],
        )).await.is_err(), "a late source cannot be re-stamped as a fresh relationship input");
        let mut old_context = memory.write_snapshot(owner).await.unwrap();
        old_context.source_event_ids.push(late);
        assert!(memory.validate_write_context(&old_context).await.is_err());

        let fresh_admission = crate::modules::memory::source::capture_source_epochs(&db, owner).await.unwrap();
        let new_epoch = fresh_admission.privacy_epoch;
        assert_eq!(fresh_admission.relationship_epoch, admitted.relationship_epoch + 1);
        assert_eq!(new_epoch, epoch + 1);
        let slow_time = chrono::Utc::now() - chrono::Duration::hours(2);
        let fresh = backend.append_sourced_message(owner, session, ChatMessage::new("user", "新接纳的消息"), slow_time, new_epoch, fresh_admission.relationship_epoch).await.unwrap();
        let mut fresh_context = memory.write_snapshot(owner).await.unwrap();
        fresh_context.source_event_ids.push(fresh);
        fresh_context.exact_source_received_at = Some((fresh, slow_time));
        memory.validate_write_context(&fresh_context).await.unwrap();

        let legacy_source = Uuid::new_v4();
        db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "INSERT INTO chat_messages(id,session_id,user_id,role,content) VALUES($1,$2,$3,'user','legacy display history')",
            vec![legacy_source.into(),session.into(),owner.to_string().into()],
        )).await.unwrap();
        assert!(db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "UPDATE chat_messages SET source_relationship_epoch=$1 WHERE id=$2",
            vec![fresh_admission.relationship_epoch.into(),legacy_source.into()],
        )).await.is_err(), "legacy display history has no recoverable relationship admission");

        // The source insert must roll back if updating its conversation fails.
        db.execute_unprepared("CREATE FUNCTION fail_source_session_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected session update failure'; END $$; CREATE TRIGGER fail_source_session_update BEFORE UPDATE ON chat_sessions FOR EACH ROW EXECUTE FUNCTION fail_source_session_update()").await.unwrap();
        assert!(backend.append_sourced_message(owner, session, ChatMessage::new("user", "must roll back"), received, epoch, admitted.relationship_epoch).await.is_err());
        assert!(backend.append_message(session, ChatMessage::new("assistant", "reply must also roll back")).await.is_err());
        let count: i64 = db.query_one(Statement::from_string(DatabaseBackend::Postgres, "SELECT COUNT(*) AS count FROM chat_messages")).await.unwrap().unwrap().try_get("", "count").unwrap();
        assert_eq!(count, 5);
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
