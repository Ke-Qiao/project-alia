use super::*;
use alia_server::expression::affect::{AffectAssistantResponse, AffectUserInput};
use sea_orm::{DatabaseBackend, Statement, TransactionTrait};

#[tokio::test]
async fn expression_writer_drains_before_owned_pool_closes_even_after_shutdown_waiter_times_out() {
    with_fixture(|mut f| async move {
        f.config.server.shutdown_grace_ms = 40;
        let runtime = ServerBuilder::from_config(Arc::new(f.config), CancellationToken::new()).build().await.unwrap();
        let state = runtime.state().clone();
        let lock = f.db.begin().await.unwrap();
        lock.execute_unprepared("SELECT pg_advisory_xact_lock(471108012)").await.unwrap();
        state.db.execute_unprepared("CREATE FUNCTION hold_appraisal_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(471108012); RETURN NEW; END $$; CREATE TRIGGER hold_appraisal_write BEFORE INSERT ON expression_appraisal_events FOR EACH ROW EXECUTE FUNCTION hold_appraisal_write()").await.unwrap();
        let owner = Uuid::new_v4().to_string();
        let observation = state.modules.expression.observe_assistant_response(AffectAssistantResponse {
            channel_id: "expression-test", user_id: &owner, response_text: "A response", route: "chat", used_tools: false, response_was_fallback: false,
        }).await;
        assert!(!observation.appraisals.is_empty());
        let handle = runtime.shutdown_handle();
        assert!(runtime.shutdown().await.unwrap_err().is::<ShutdownIncomplete>());
        assert!(!handle.is_finished());
        assert!(handle.pending_modules().iter().any(|name| name == "expression_persistence"));
        state.db.ping().await.unwrap();
        lock.rollback().await.unwrap();
        tokio::time::timeout(Duration::from_secs(4), handle.wait()).await.unwrap().unwrap();
        assert!(handle.is_finished());
        assert!(state.db.ping().await.is_err());
        let count: i64 = f.db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
            "SELECT count(*) AS count FROM expression_appraisal_events WHERE user_id=$1", [owner.into()],
        )).await.unwrap().unwrap().try_get("", "count").unwrap();
        assert_eq!(count as usize, observation.appraisals.len());
    }).await;
}

#[tokio::test]
async fn expression_writer_failure_is_reported_after_join_and_never_blocks_pool_cleanup() {
    with_fixture(|f| async move {
        let runtime = ServerBuilder::from_config(Arc::new(f.config), CancellationToken::new()).build().await.unwrap();
        let state = runtime.state().clone();
        state.db.execute_unprepared("CREATE FUNCTION fail_appraisal_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected appraisal failure'; END $$; CREATE TRIGGER fail_appraisal_write BEFORE INSERT ON expression_appraisal_events FOR EACH ROW EXECUTE FUNCTION fail_appraisal_write()").await.unwrap();
        let owner = Uuid::new_v4().to_string();
        let observation = state.modules.expression.observe_assistant_response(AffectAssistantResponse {
            channel_id: "expression-test", user_id: &owner, response_text: "A response", route: "chat", used_tools: false, response_was_fallback: false,
        }).await;
        assert!(!observation.appraisals.is_empty());
        let handle = runtime.shutdown_handle();
        let error = runtime.shutdown().await.unwrap_err();
        assert!(error.to_string().contains("expression_persistence"));
        assert!(handle.is_finished());
        assert!(state.db.ping().await.is_err());
        assert!(state.modules.expression.shutdown().await.is_err());
        assert!(handle.wait().await.is_err());
    }).await;
}

async fn block_appraisal_insert(db: &DatabaseConnection) -> (sea_orm::DatabaseTransaction, i64) {
    let key = (Uuid::new_v4().as_u128() as u32 & 0x7fff_ffff) as i64;
    let lock = db.begin().await.unwrap();
    lock.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock($1)",
        [key.into()],
    ))
    .await
    .unwrap();
    db.execute_unprepared(&format!("CREATE FUNCTION hold_relationship_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; END $$; CREATE TRIGGER hold_relationship_write BEFORE INSERT ON expression_appraisal_events FOR EACH ROW EXECUTE FUNCTION hold_relationship_write()")).await.unwrap();
    (lock, key)
}

async fn wait_insert_blocked(db: &DatabaseConnection, key: i64) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let blocked: bool = db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND objid::bigint=$1 AND NOT granted) AS blocked", [key.into()])).await.unwrap().unwrap().try_get("", "blocked").unwrap();
            if blocked { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap();
}

async fn private_rows(db: &DatabaseConnection, user: &str) -> i64 {
    db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "SELECT count(*) AS count FROM expression_appraisal_events WHERE user_id=$1 AND scope IN ('user','relationship')", [user.into()])).await.unwrap().unwrap().try_get("", "count").unwrap()
}

async fn wait_ram_clear(
    expression: &alia_server::expression::ExpressionRuntime,
    user: &str,
    baseline: f32,
) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while expression.relationship_profile(user).trust != baseline {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn relationship_clear_waits_for_older_insert_and_restart_does_not_restore_it() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let expression = runtime.state().modules.expression.clone();
        let user = Uuid::new_v4().to_string();
        let baseline = expression.relationship_profile(&user).trust;
        let (lock, key) = block_appraisal_insert(&f.db).await;
        expression
            .observe_user_input(AffectUserInput {
                channel_id: "clear-test",
                user_id: &user,
                user_text: "thank you",
            })
            .await;
        assert!(expression.relationship_profile(&user).trust > baseline);
        wait_insert_blocked(&f.db, key).await;
        let (clear_owner, clear_user) = (expression.clone(), user.clone());
        let clear =
            tokio::spawn(async move { clear_owner.clear_relationship_data(&clear_user).await });
        wait_ram_clear(&expression, &user, baseline).await;
        assert!(
            !clear.is_finished(),
            "clear cannot ACK while its older INSERT is still executing"
        );
        lock.rollback().await.unwrap();
        let result = clear.await.unwrap().unwrap();
        assert!(result.persisted_events_removed > 0);
        assert_eq!(private_rows(&f.db, &user).await, 0);
        runtime.shutdown().await.unwrap();
        let restarted = f.builder().build().await.unwrap();
        assert!(
            (restarted
                .state()
                .modules
                .expression
                .affect_snapshot("clear-test", &user)
                .await
                .social
                .trust
                - baseline)
                .abs()
                < 0.00001
        );
        restarted.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn dropping_clear_waiter_does_not_cancel_its_admitted_deletion() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let expression = runtime.state().modules.expression.clone();
        let user = Uuid::new_v4().to_string();
        let baseline = expression.relationship_profile(&user).trust;
        let (lock, key) = block_appraisal_insert(&f.db).await;
        expression
            .observe_user_input(AffectUserInput {
                channel_id: "clear-test",
                user_id: &user,
                user_text: "thank you",
            })
            .await;
        wait_insert_blocked(&f.db, key).await;
        let (clear_owner, clear_user) = (expression.clone(), user.clone());
        let clear =
            tokio::spawn(async move { clear_owner.clear_relationship_data(&clear_user).await });
        wait_ram_clear(&expression, &user, baseline).await;
        clear.abort();
        assert!(clear.await.unwrap_err().is_cancelled());
        lock.rollback().await.unwrap();
        runtime.shutdown().await.unwrap();
        assert_eq!(private_rows(&f.db, &user).await, 0);
    })
    .await;
}

#[tokio::test]
async fn failed_relationship_delete_never_acknowledges_success() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let expression = runtime.state().modules.expression.clone();
        let user = Uuid::new_v4().to_string();
        expression.observe_user_input(AffectUserInput { channel_id:"clear-test", user_id:&user, user_text:"thank you" }).await;
        tokio::time::timeout(Duration::from_secs(1), async { while private_rows(&f.db, &user).await == 0 { tokio::task::yield_now().await; } }).await.unwrap();
        f.db.execute_unprepared("CREATE FUNCTION fail_relationship_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected relationship delete failure'; END $$; CREATE TRIGGER fail_relationship_delete BEFORE DELETE ON expression_appraisal_events FOR EACH ROW EXECUTE FUNCTION fail_relationship_delete()").await.unwrap();
        let error = expression.clear_relationship_data(&user).await.unwrap_err();
        assert!(error.to_string().contains("injected relationship delete failure"));
        assert!(private_rows(&f.db, &user).await > 0);
        assert!(runtime.shutdown().await.is_err());
    }).await;
}

#[tokio::test]
async fn full_or_closed_writer_rejects_clear_before_mutating_ram() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let expression = runtime.state().modules.expression.clone();
        let user = Uuid::new_v4().to_string();
        let (lock, key) = block_appraisal_insert(&f.db).await;
        expression
            .observe_user_input(AffectUserInput {
                channel_id: "clear-test",
                user_id: &user,
                user_text: "thank you",
            })
            .await;
        wait_insert_blocked(&f.db, key).await;
        let mut rejected = false;
        for _ in 0..140 {
            let observation = expression
                .observe_user_input(AffectUserInput {
                    channel_id: "clear-test",
                    user_id: &user,
                    user_text: "thank you",
                })
                .await;
            if observation.appraisals.is_empty() {
                assert!(
                    observation
                        .persistence_error
                        .as_deref()
                        .is_some_and(|error| error.contains("queue is full"))
                );
                rejected = true;
                break;
            }
        }
        assert!(rejected, "the persistence queue must be bounded");
        let before = expression.relationship_profile(&user);
        assert!(
            expression
                .clear_relationship_data(&user)
                .await
                .unwrap_err()
                .to_string()
                .contains("queue is full")
        );
        assert_eq!(
            expression.relationship_profile(&user).updated_at,
            before.updated_at
        );
        lock.rollback().await.unwrap();
        assert!(
            runtime.shutdown().await.is_err(),
            "rejected admissions remain reported at shutdown"
        );
        assert!(
            expression
                .clear_relationship_data(&user)
                .await
                .unwrap_err()
                .to_string()
                .contains("closed")
        );
        assert_eq!(
            expression.relationship_profile(&user).updated_at,
            before.updated_at
        );
    })
    .await;
}

#[tokio::test]
async fn clear_ack_deadline_is_an_error_while_its_command_remains_owned() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let expression = runtime.state().modules.expression.clone();
        let user = Uuid::new_v4().to_string();
        let (lock, key) = block_appraisal_insert(&f.db).await;
        expression
            .observe_user_input(AffectUserInput {
                channel_id: "clear-test",
                user_id: &user,
                user_text: "thank you",
            })
            .await;
        wait_insert_blocked(&f.db, key).await;
        let error = tokio::time::timeout(
            Duration::from_secs(3),
            expression.clear_relationship_data(&user),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("acknowledgement timed out"));
        // The blocked SQL statement may outlive the local write timeout. Its
        // transaction lock prevents clear from overtaking its eventual rollback.
        lock.rollback().await.unwrap();
        assert!(
            runtime.shutdown().await.is_err(),
            "the earlier unconfirmed write is still reported"
        );
        assert_eq!(private_rows(&f.db, &user).await, 0);
        let restarted = f.builder().build().await.unwrap();
        let snapshot = restarted
            .state()
            .modules
            .expression
            .affect_snapshot("clear-test", &user)
            .await;
        assert!(
            !snapshot
                .dominant_causes
                .iter()
                .any(|kind| kind.contains("relationship"))
        );
        restarted.shutdown().await.unwrap();
    })
    .await;
}
