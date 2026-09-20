use super::*;
use alia_server::expression::{AppraisalEvent, ExpressionRuntime, rule_based_user_appraisals};
use alia_server::modules::memory::context::{AppraisalInput, AppraisalReceipt, ContextSource};
use alia_server::modules::memory::source::capture_source_epochs;
use alia_server::state::AppState;
use sea_orm::{DatabaseBackend, Statement, TransactionTrait};

fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}

async fn input(state: &AppState, owner: Uuid) -> AppraisalInput {
    let text = "thank you";
    let session = state
        .modules
        .conversation
        .create_session("expression-test", &owner.to_string())
        .await
        .unwrap();
    let epochs = capture_source_epochs(&state.db, owner).await.unwrap();
    let source = Uuid::new_v4();
    let row = state.db.query_one(sql(
        "INSERT INTO chat_messages(id,session_id,user_id,role,content,created_at,source_received_at,source_privacy_epoch,source_relationship_epoch) VALUES($1,$2,$3,'user',$4,clock_timestamp(),clock_timestamp(),$5,$6) RETURNING source_received_at",
        vec![source.into(),session.into(),owner.to_string().into(),text.into(),epochs.privacy_epoch.into(),epochs.relationship_epoch.into()],
    )).await.unwrap().unwrap();
    let generation = state
        .modules
        .context
        .materialize(
            ContextSource {
                owner_id: owner,
                request_id: Uuid::new_v4(),
                channel_id: "expression-test".into(),
                conversation_id: Some(session),
                source_event_id: Some(source),
                source_received_at: row.try_get("", "source_received_at").unwrap(),
                source_privacy_epoch: epochs.privacy_epoch,
            },
            text,
        )
        .await
        .unwrap();
    state
        .modules
        .context
        .prepare_user_appraisal(&generation)
        .await
        .unwrap()
}

fn drafts(input: &AppraisalInput) -> Vec<AppraisalEvent> {
    let result =
        rule_based_user_appraisals(input.text(), input.effective_at(), &input.policy().affect);
    assert!(!result.is_empty());
    result
}

fn submit(
    expression: Arc<ExpressionRuntime>,
    input: AppraisalInput,
) -> tokio::task::JoinHandle<anyhow::Result<AppraisalReceipt>> {
    let drafts = drafts(&input);
    tokio::spawn(async move { expression.submit_appraisal(input, drafts).await })
}

async fn block_appraisal(
    db: &DatabaseConnection,
    action: &str,
) -> (sea_orm::DatabaseTransaction, i64) {
    assert!(matches!(action, "INSERT" | "DELETE"));
    let key = (Uuid::new_v4().as_u128() as u32 & 0x7fff_ffff) as i64;
    let lock = db.begin().await.unwrap();
    lock.execute(sql("SELECT pg_advisory_xact_lock($1)", vec![key.into()]))
        .await
        .unwrap();
    db.execute_unprepared(&format!("CREATE FUNCTION hold_appraisal_mutation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); IF TG_OP='DELETE' THEN RETURN OLD; END IF; RETURN NEW; END $$; CREATE TRIGGER hold_appraisal_mutation BEFORE {action} ON appraisal_records FOR EACH ROW EXECUTE FUNCTION hold_appraisal_mutation()")).await.unwrap();
    (lock, key)
}

async fn wait_statement_blocked(db: &DatabaseConnection, key: i64) -> i32 {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Some(row) = db.query_one(sql("SELECT pid FROM pg_locks WHERE locktype='advisory' AND objid::bigint=$1 AND NOT granted", vec![key.into()])).await.unwrap() {
                return row.try_get("", "pid").unwrap();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("canonical SQL did not reach the fixture gate")
}

async fn wait_blocked_by(db: &DatabaseConnection, pid: i32) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let blocked: bool = db.query_one(sql("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))) AS blocked", vec![pid.into()])).await.unwrap().unwrap().try_get("", "blocked").unwrap();
            if blocked { return; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("clear must wait for the writer's authority lock");
}

async fn records(db: &DatabaseConnection, owner: Uuid) -> i64 {
    db.query_one(sql(
        "SELECT count(*) AS n FROM appraisal_records WHERE owner_id=$1",
        vec![owner.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "n")
    .unwrap()
}

async fn epoch(db: &DatabaseConnection, owner: Uuid) -> i64 {
    db.query_one(sql(
        "SELECT relationship_epoch FROM relationship_authority WHERE owner_id=$1",
        vec![owner.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "relationship_epoch")
    .unwrap()
}

async fn operations(db: &DatabaseConnection, owner: Uuid) -> i64 {
    db.query_one(sql(
        "SELECT count(*) AS n FROM relationship_clear_operations WHERE owner_id=$1",
        vec![owner.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "n")
    .unwrap()
}

#[tokio::test]
async fn expression_writer_drains_before_owned_pool_closes_even_after_shutdown_waiter_times_out() {
    with_fixture(|mut f| async move {
        f.config.server.shutdown_grace_ms = 40;
        let runtime = ServerBuilder::from_config(Arc::new(f.config), CancellationToken::new()).build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, _) = delivery::account(&state, "user").await;
        let input = input(&state, owner).await;
        let id = input.id();
        let expected_events = drafts(&input).len();
        let (lock, key) = block_appraisal(&f.db, "INSERT").await;
        let waiting = submit(state.modules.expression.clone(), input);
        wait_statement_blocked(&f.db, key).await;
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());

        let handle = runtime.shutdown_handle();
        assert!(runtime.shutdown().await.unwrap_err().is::<ShutdownIncomplete>());
        assert!(!handle.is_finished());
        assert!(handle.pending_modules().iter().any(|name| name == "expression_persistence"));
        state.db.ping().await.unwrap();
        lock.rollback().await.unwrap();
        tokio::time::timeout(Duration::from_secs(4), handle.wait()).await.unwrap().unwrap();
        assert!(handle.is_finished());
        assert!(state.db.ping().await.is_err());
        let row = f.db.query_one(sql("SELECT jsonb_array_length(events) AS n FROM appraisal_records WHERE appraisal_id=$1 AND owner_id=$2", vec![id.into(),owner.into()])).await.unwrap().unwrap();
        assert_eq!(row.try_get::<i32>("", "n").unwrap() as usize, expected_events);
        assert_eq!(records(&f.db, owner).await, 1);
    }).await;
}

#[tokio::test]
async fn expression_writer_failure_is_reported_after_join_and_never_blocks_pool_cleanup() {
    with_fixture(|f| async move {
        let runtime = ServerBuilder::from_config(Arc::new(f.config), CancellationToken::new()).build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, _) = delivery::account(&state, "user").await;
        let input = input(&state, owner).await;
        state.db.execute_unprepared("CREATE FUNCTION fail_appraisal_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected appraisal failure'; END $$; CREATE TRIGGER fail_appraisal_write BEFORE INSERT ON appraisal_records FOR EACH ROW EXECUTE FUNCTION fail_appraisal_write()").await.unwrap();
        let error = state.modules.expression.submit_appraisal(input.clone(), drafts(&input)).await.unwrap_err();
        assert!(error.to_string().contains("injected appraisal failure"));
        assert_eq!(records(&f.db, owner).await, 0);
        let handle = runtime.shutdown_handle();
        let error = runtime.shutdown().await.unwrap_err();
        assert!(error.to_string().contains("expression_persistence"));
        assert!(handle.is_finished());
        assert!(state.db.ping().await.is_err());
        assert!(state.modules.expression.shutdown().await.is_err());
        assert!(handle.wait().await.is_err());
    }).await;
}

#[tokio::test]
async fn relationship_clear_waits_for_older_insert_and_restart_does_not_restore_it() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, _) = delivery::account(&state, "user").await;
        let input = input(&state, owner).await;
        let baseline = state
            .modules
            .context
            .relationship_profile(owner)
            .await
            .unwrap();
        let (lock, key) = block_appraisal(&f.db, "INSERT").await;
        let write = submit(state.modules.expression.clone(), input);
        let writer_pid = wait_statement_blocked(&f.db, key).await;
        let service = state.modules.context.clone();
        let operation = Uuid::new_v4();
        let clear = tokio::spawn(async move {
            service
                .clear_relationship(owner, operation, baseline.relationship_epoch)
                .await
        });
        wait_blocked_by(&f.db, writer_pid).await;
        assert!(
            !clear.is_finished(),
            "clear cannot ACK before the older INSERT releases authority"
        );
        lock.rollback().await.unwrap();
        let written = write.await.unwrap().unwrap();
        assert!(written.event_count > 0);
        let cleared = clear.await.unwrap().unwrap();
        assert_eq!(cleared.removed_appraisals, 1);
        assert_eq!(records(&f.db, owner).await, 0);
        assert_eq!(epoch(&f.db, owner).await, baseline.relationship_epoch + 1);
        assert_eq!(
            state
                .modules
                .context
                .clear_relationship(owner, operation, baseline.relationship_epoch)
                .await
                .unwrap(),
            cleared
        );
        runtime.shutdown().await.unwrap();
        let restarted = f.builder().build().await.unwrap();
        let profile = restarted
            .state()
            .modules
            .context
            .relationship_profile(owner)
            .await
            .unwrap();
        assert!((profile.trust - baseline.trust).abs() < 0.00001);
        assert_eq!(profile.relationship_epoch, cleared.relationship_epoch);
        restarted.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn dropping_clear_before_commit_rolls_back_and_same_operation_can_retry() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, _) = delivery::account(&state, "user").await;
        let input = input(&state, owner).await;
        state.modules.expression.submit_appraisal(input.clone(), drafts(&input)).await.unwrap();
        let original_epoch = epoch(&f.db, owner).await;
        let operation = Uuid::new_v4();
        let (lock, key) = block_appraisal(&f.db, "DELETE").await;
        let service = state.modules.context.clone();
        let clear = tokio::spawn(async move { service.clear_relationship(owner, operation, original_epoch).await });
        wait_statement_blocked(&f.db, key).await;
        clear.abort();
        assert!(clear.await.unwrap_err().is_cancelled());
        lock.rollback().await.unwrap();
        f.db.execute_unprepared("DROP TRIGGER hold_appraisal_mutation ON appraisal_records; DROP FUNCTION hold_appraisal_mutation()").await.unwrap();
        let unchanged = state.modules.context.relationship_profile(owner).await.unwrap();
        assert_eq!(unchanged.relationship_epoch, original_epoch);
        assert_eq!(records(&f.db, owner).await, 1);
        assert_eq!(operations(&f.db, owner).await, 0);
        let cleared = state.modules.context.clear_relationship(owner, operation, original_epoch).await.unwrap();
        assert_eq!(cleared.removed_appraisals, 1);
        assert_eq!(cleared.relationship_epoch, original_epoch + 1);
        assert_eq!(records(&f.db, owner).await, 0);
        assert_eq!(operations(&f.db, owner).await, 1);
        runtime.shutdown().await.unwrap();
    }).await;
}

#[tokio::test]
async fn failed_relationship_delete_never_acknowledges_success() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, _) = delivery::account(&state, "user").await;
        let input = input(&state, owner).await;
        state.modules.expression.submit_appraisal(input.clone(), drafts(&input)).await.unwrap();
        let original_epoch = epoch(&f.db, owner).await;
        f.db.execute_unprepared("CREATE FUNCTION fail_relationship_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected relationship delete failure'; END $$; CREATE TRIGGER fail_relationship_delete BEFORE DELETE ON appraisal_records FOR EACH ROW EXECUTE FUNCTION fail_relationship_delete()").await.unwrap();
        let error = state.modules.context.clear_relationship(owner, Uuid::new_v4(), original_epoch).await.unwrap_err();
        assert!(error.to_string().contains("injected relationship delete failure"));
        assert_eq!(records(&f.db, owner).await, 1);
        assert_eq!(epoch(&f.db, owner).await, original_epoch);
        assert_eq!(operations(&f.db, owner).await, 0);
        // Clear owns a separate failed transaction, not a writer queue command.
        runtime.shutdown().await.unwrap();
    }).await;
}

#[tokio::test]
async fn full_or_closed_writer_rejects_new_appraisal_without_publishing_it() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, _) = delivery::account(&state, "user").await;
        let input = input(&state, owner).await;
        let appraisal = input.id();
        let events = drafts(&input);
        let (lock, key) = block_appraisal(&f.db, "INSERT").await;
        let first = submit(state.modules.expression.clone(), input.clone());
        wait_statement_blocked(&f.db, key).await;
        let mut waiting = tokio::task::JoinSet::new();
        // One immutable input makes accepted duplicates observable as the same
        // receipt, without inventing new facts just to fill the queue.
        for _ in 0..129 {
            let expression = state.modules.expression.clone();
            let (input, events) = (input.clone(), events.clone());
            waiting.spawn(async move { expression.submit_appraisal(input, events).await });
        }
        let rejected = tokio::time::timeout(Duration::from_secs(1), waiting.join_next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(rejected.to_string().contains("queue full"));
        assert_eq!(records(&f.db, owner).await, 0);
        lock.rollback().await.unwrap();
        assert_eq!(first.await.unwrap().unwrap().appraisal_id, appraisal);
        while let Some(result) = waiting.join_next().await {
            assert_eq!(result.unwrap().unwrap().appraisal_id, appraisal);
        }
        runtime.shutdown().await.unwrap();
        assert_eq!(records(&f.db, owner).await, 1);
        let closed = state
            .modules
            .expression
            .submit_appraisal(input, events)
            .await
            .unwrap_err();
        assert!(closed.to_string().contains("closed"));
        assert_eq!(records(&f.db, owner).await, 1);
    })
    .await;
}

#[tokio::test]
async fn appraisal_ack_deadline_reports_unknown_and_clear_fences_the_original_identity() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, _) = delivery::account(&state, "user").await;
        let input = input(&state, owner).await;
        let baseline = state
            .modules
            .context
            .relationship_profile(owner)
            .await
            .unwrap();
        let (lock, key) = block_appraisal(&f.db, "INSERT").await;
        let write = submit(state.modules.expression.clone(), input.clone());
        wait_statement_blocked(&f.db, key).await;
        let error = tokio::time::timeout(Duration::from_secs(3), write)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("acknowledgement"));
        assert!(error.to_string().contains(&input.id().to_string()));
        lock.rollback().await.unwrap();
        let clear = state
            .modules
            .context
            .clear_relationship(owner, Uuid::new_v4(), baseline.relationship_epoch)
            .await
            .unwrap();
        assert_eq!(clear.relationship_epoch, baseline.relationship_epoch + 1);
        assert!(
            runtime.shutdown().await.is_err(),
            "unconfirmed writer commits remain visible after join"
        );
        assert_eq!(records(&f.db, owner).await, 0);
        let restarted = f.builder().build().await.unwrap();
        let profile = restarted
            .state()
            .modules
            .context
            .relationship_profile(owner)
            .await
            .unwrap();
        assert!((profile.trust - baseline.trust).abs() < 0.00001);
        assert_eq!(profile.relationship_epoch, clear.relationship_epoch);
        let error = restarted
            .state()
            .modules
            .expression
            .submit_appraisal(input.clone(), drafts(&input))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("relationship"));
        assert_eq!(records(&f.db, owner).await, 0);
        restarted.shutdown().await.unwrap();
    })
    .await;
}
