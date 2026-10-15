//! Dropping the whole constructor must reclaim its already-created resources.
use super::delivery_support::{FixtureInitialization, InitializationStep};
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
    TransactionTrait,
};
use std::{
    future::Future,
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Wake, Waker},
    time::Duration,
};

struct CheckpointDrop(Arc<AtomicBool>);

impl Drop for CheckpointDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

async fn cancelled_initialization(step: InitializationStep) {
    let mut options = ConnectOptions::new(std::env::var("ALIA_TEST_DATABASE_URL").unwrap());
    options.max_connections(1).sqlx_logging(false);
    let observer = Database::connect(options).await.unwrap();
    let evidence = Arc::new(Mutex::new(None));
    let observed = evidence.clone();
    let checkpoint_dropped = Arc::new(AtomicBool::new(false));
    let dropped = checkpoint_dropped.clone();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let mut entered_tx = Some(entered_tx);
    let mut initialization = FixtureInitialization::start_with(move |current, fixture| {
        let observed = observed.clone();
        let dropped = dropped.clone();
        let entered_tx = (current == step).then(|| entered_tx.take().unwrap());
        Box::pin(async move {
            let Some(entered_tx) = entered_tx else {
                return;
            };
            let _drop = CheckpointDrop(dropped);
            *observed.lock().unwrap() =
                Some((fixture.schema_name().to_string(), fixture.db.clone()));
            entered_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        })
    });
    let completion = initialization.completion();
    let retained_owner = match step {
        InitializationStep::WorkPool => {
            let mut construction = Box::pin(async move { initialization.ready().await });
            tokio::select! {
                _ = &mut construction => panic!("constructor returned before cancellation checkpoint"),
                entered = entered_rx => entered.unwrap(),
            }
            // Drop the entire owner and its pending ready waiter.
            drop(construction);
            None
        }
        InitializationStep::UserSeed => {
            let ready = initialization.ready();
            entered_rx.await.unwrap();
            // Even a waiter that was never polled must request cancellation.
            drop(ready);
            Some(initialization)
        }
        _ => unreachable!("this probe covers the two resource boundaries"),
    };
    let joined = completion.await;
    drop(retained_owner);
    let (schema, work_pool) = evidence.lock().unwrap().take().unwrap();
    let stage = match step {
        InitializationStep::WorkPool => "after_schema",
        InitializationStep::UserSeed => "after_work_pool",
        _ => unreachable!("this probe covers the two resource boundaries"),
    };
    let (schema_remains, work_pool_remains) =
        observe_then_cleanup(observer, schema, work_pool, stage).await;
    assert!(joined.is_ok(), "cancelled initialization owner failed");
    assert!(
        checkpoint_dropped.load(Ordering::SeqCst),
        "constructor checkpoint was not dropped"
    );
    assert!(
        !schema_remains,
        "cancelled initialization left its schema behind"
    );
    assert!(
        !work_pool_remains,
        "cancelled initialization left its work pool open"
    );
}

async fn observe_then_cleanup(
    observer: DatabaseConnection,
    schema: String,
    work_pool: DatabaseConnection,
    stage: &str,
) -> (bool, bool) {
    let schema_query = observer
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname=$1) AS present",
            [schema.clone().into()],
        ))
        .await;
    let work_pool_remains = work_pool.execute_unprepared("SELECT 1").await.is_ok();
    let schema_remains = schema_query
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .and_then(|row| row.try_get::<bool>("", "present").ok());
    eprintln!(
        "initialization cancellation: stage={stage}, schema_remains={schema_remains:?}, work_pool_remains={work_pool_remains}",
    );
    // Preserve any leak evidence before reclaiming the test's own resources.
    let work_pool_cleanup = if matches!(work_pool, DatabaseConnection::Disconnected) {
        Ok(())
    } else {
        work_pool.close().await
    };
    assert!(
        schema
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    );
    let schema_cleanup = observer
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .await;
    let observer_cleanup = observer.close().await;
    assert!(
        work_pool_cleanup.is_ok(),
        "fallback work pool cleanup failed"
    );
    assert!(schema_cleanup.is_ok(), "fallback schema cleanup failed");
    assert!(observer_cleanup.is_ok(), "observer cleanup failed");
    (
        schema_remains.expect("schema observation failed"),
        work_pool_remains,
    )
}

async fn waiting_create_backend(observer: &DatabaseConnection, query: &str) -> Result<i32, String> {
    // This bounds observation of an actual database lock, not fixture execution.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let row = observer
                .query_one(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND query=$1 AND state='active' AND wait_event_type='Lock'",
                    [query.to_owned().into()],
                ))
                .await
                .map_err(|error| format!("observe CREATE lock: {error}"))?;
            if let Some(row) = row {
                return row.try_get("", "pid").map_err(|error| format!("read CREATE pid: {error}"));
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| "CREATE lock was not observed within the observation bound".to_string())?
}

async fn waiting_backend_exit(observer: &DatabaseConnection, pid: i32) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let row = observer
                .query_one(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1) AS present",
                    [pid.into()],
                ))
                .await
                .map_err(|error| format!("observe CREATE backend exit: {error}"))?
                .ok_or("backend exit observation returned no row")?;
            let present: bool = row
                .try_get("", "present")
                .map_err(|error| format!("read backend presence: {error}"))?;
            if !present {
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| "CREATE backend did not exit within the observation bound".to_string())?
}

#[tokio::test]
async fn cancelled_inflight_create_is_drained_before_schema_cleanup() {
    let mut options = ConnectOptions::new(std::env::var("ALIA_TEST_DATABASE_URL").unwrap());
    options.max_connections(1).sqlx_logging(false);
    let observer = Database::connect(options.clone()).await.unwrap();
    let blocker = Database::connect(options).await.unwrap();
    let evidence = Arc::new(Mutex::new(None));
    let observed = evidence.clone();
    let later_checkpoint = Arc::new(AtomicBool::new(false));
    let later = later_checkpoint.clone();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let mut gate = Some((entered_tx, release_rx));
    let mut initialization = FixtureInitialization::start_with(move |current, fixture| {
        let observed = observed.clone();
        let later = later.clone();
        let gate = (current == InitializationStep::CreateSchema).then(|| gate.take().unwrap());
        Box::pin(async move {
            let Some((entered_tx, release_rx)) = gate else {
                later.store(true, Ordering::SeqCst);
                return;
            };
            *observed.lock().unwrap() =
                Some((fixture.schema_name().to_string(), fixture.db.clone()));
            entered_tx.send(()).unwrap();
            release_rx.await.unwrap();
        })
    });
    let completion = initialization.completion();
    let mut ready = Box::pin(initialization.ready());
    tokio::select! {
        _ = &mut ready => panic!("constructor returned before CREATE checkpoint"),
        entered = entered_rx => entered.unwrap(),
    }
    let (schema, work_pool) = evidence.lock().unwrap().take().unwrap();
    assert!(
        schema
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    );
    let create = format!("CREATE SCHEMA {schema}");
    let transaction = blocker.begin().await.unwrap();
    let blocker_create = transaction.execute_unprepared(&create).await;
    let released = release_tx.send(());
    let locked = waiting_create_backend(&observer, &create).await;
    // The real CREATE is now in flight. Cancel its waiter, not the SQL operation.
    drop(ready);
    let locked_after_cancel = waiting_create_backend(&observer, &create).await;
    let mut close_waiter = Box::pin(AssertUnwindSafe(initialization.close()).catch_unwind());
    let close_pending = futures::poll!(close_waiter.as_mut()).is_pending();
    drop(close_waiter);
    let mut completion_waiter = Box::pin(completion.clone());
    let completion_pending = futures::poll!(completion_waiter.as_mut()).is_pending();
    drop(completion_waiter);
    // Record observations first; release the blocker and drain the original
    // owner before asserting, so a failed observation cannot strand CREATE.
    let rollback = transaction.rollback().await;
    let blocker_closed = blocker.close().await;
    let joined = completion.await;
    drop(initialization);
    let backend_exit = if let Ok(pid) = locked.as_ref() {
        waiting_backend_exit(&observer, *pid).await
    } else {
        Err("CREATE backend was not observed".to_string())
    };
    let (schema_remains, work_pool_remains) =
        observe_then_cleanup(observer, schema, work_pool, "inflight_create").await;
    eprintln!(
        "inflight CREATE cancellation: lock_observed={}, same_backend_waiting={}, close_pending={close_pending}, completion_pending={completion_pending}, backend_exited={}",
        locked.is_ok(),
        matches!((&locked, &locked_after_cancel), (Ok(before), Ok(after)) if before == after),
        backend_exit.is_ok(),
    );
    assert!(blocker_create.is_ok(), "blocking schema was not created");
    assert!(released.is_ok(), "CREATE checkpoint was not released");
    assert!(rollback.is_ok(), "blocking transaction rollback failed");
    assert!(blocker_closed.is_ok(), "blocker pool cleanup failed");
    assert!(joined.is_ok(), "cancelled initialization owner failed");
    let locked = locked.expect("actual CREATE was not observed waiting for its schema lock");
    assert_eq!(
        locked_after_cancel.ok(),
        Some(locked),
        "cancellation dropped the original CREATE"
    );
    assert!(close_pending, "close did not retain the in-flight CREATE");
    assert!(
        completion_pending,
        "completion finished while CREATE was blocked"
    );
    backend_exit.expect("CREATE backend survived owner completion");
    assert!(
        !later_checkpoint.load(Ordering::SeqCst),
        "cancelled constructor continued to later stages"
    );
    assert!(
        !schema_remains,
        "late CREATE survived initialization cleanup"
    );
    assert!(!work_pool_remains, "cancelled CREATE left a work pool open");
}

struct ReadyWake(Arc<tokio::sync::Notify>);

impl Wake for ReadyWake {
    fn wake(self: Arc<Self>) {
        self.0.notify_one();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.notify_one();
    }
}

#[tokio::test]
async fn cancelled_buffered_ready_cannot_be_retried_as_a_closed_fixture() {
    let mut options = ConnectOptions::new(std::env::var("ALIA_TEST_DATABASE_URL").unwrap());
    options.max_connections(1).sqlx_logging(false);
    let observer = Database::connect(options).await.unwrap();
    let evidence = Arc::new(Mutex::new(None));
    let observed = evidence.clone();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let mut gate = Some((entered_tx, release_rx));
    let mut initialization = FixtureInitialization::start_with(move |current, fixture| {
        let observed = observed.clone();
        let gate = if current == InitializationStep::UserSeed {
            gate.take()
        } else {
            None
        };
        Box::pin(async move {
            if let Some((entered_tx, release_rx)) = gate {
                *observed.lock().unwrap() =
                    Some((fixture.schema_name().to_string(), fixture.db.clone()));
                let _ = entered_tx.send(());
                let _ = release_rx.await;
            }
        })
    });
    let completion = initialization.completion();
    let notification = Arc::new(tokio::sync::Notify::new());
    let waker = Waker::from(Arc::new(ReadyWake(notification.clone())));
    let mut context = Context::from_waker(&waker);
    let mut ready = Box::pin(initialization.ready());
    // Register exactly one receiver waker. Once it fires, the ready message is
    // buffered, but this waiter has not polled again to consume that message.
    let initially_pending = ready.as_mut().poll(&mut context).is_pending();
    // Admission and initialization precede the existing ready observation
    // window. Synchronize on the actual stage instead of timing queue wait.
    let entered = entered_rx.await;
    let released = release_tx.send(());
    let notified = if entered.is_ok() && released.is_ok() {
        tokio::time::timeout(Duration::from_secs(5), notification.notified())
            .await
            .is_ok()
    } else {
        false
    };
    let mut completion_waiter = Box::pin(completion.clone());
    let owner_pending = futures::poll!(completion_waiter.as_mut()).is_pending();
    drop(completion_waiter);
    drop(ready);
    let retry = AssertUnwindSafe(initialization.ready())
        .catch_unwind()
        .await;
    let retry_message = match retry {
        Ok(fixture) => {
            drop(fixture);
            None
        }
        Err(error) => error
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| error.downcast_ref::<&str>().map(|text| (*text).to_string())),
    };
    let joined = completion.await;
    drop(initialization);
    let resource_evidence = evidence.lock().unwrap().take();
    let (resources, observer_closed) = if let Some((schema, work_pool)) = resource_evidence {
        (
            Some(observe_then_cleanup(observer, schema, work_pool, "buffered_ready").await),
            true,
        )
    } else {
        // A failed handshake can precede schema creation. The joined owner is
        // responsible for its resources; always close our independent observer.
        (None, observer.close().await.is_ok())
    };
    eprintln!(
        "buffered ready cancellation: initially_pending={initially_pending}, entered={}, released={}, notified={notified}, owner_pending={owner_pending}, retry_cancelled={}",
        entered.is_ok(),
        released.is_ok(),
        retry_message.as_deref() == Some("Delivery database initialization cancelled"),
    );
    assert!(observer_closed, "observer cleanup failed");
    assert!(joined.is_ok(), "cancelled initialization owner failed");
    assert!(initially_pending, "ready was consumed on its first poll");
    assert!(
        entered.is_ok(),
        "constructor did not reach the UserSeed gate"
    );
    assert!(released.is_ok(), "UserSeed gate was not released");
    assert!(notified, "ready send did not wake its registered waiter");
    assert!(
        owner_pending,
        "owner exited before buffered ready cancellation"
    );
    assert_eq!(
        retry_message.as_deref(),
        Some("Delivery database initialization cancelled")
    );
    let (schema_remains, work_pool_remains) =
        resources.expect("UserSeed resource evidence missing");
    assert!(
        !schema_remains,
        "cancelled buffered ready left its schema behind"
    );
    assert!(
        !work_pool_remains,
        "cancelled buffered ready left its work pool open"
    );
}

#[tokio::test]
async fn cancelled_after_schema_creation_removes_schema() {
    cancelled_initialization(InitializationStep::WorkPool).await;
}

#[tokio::test]
async fn cancelled_after_work_pool_creation_closes_pool_and_removes_schema() {
    cancelled_initialization(InitializationStep::UserSeed).await;
}
