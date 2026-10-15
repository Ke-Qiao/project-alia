//! Exercise the actual channel-init SQL await with its paired owner already saved.
use super::*;
use sea_orm::{ConnectionTrait, DatabaseTransaction, TransactionTrait};

#[derive(Default)]
struct PartialRuntime {
    runtime: Option<Runtime>,
    lock: Option<DatabaseTransaction>,
}

async fn partial_initialization(cancel: bool) {
    let cleaned = Arc::new(AtomicBool::new(false));
    let cleanup_checked = cleaned.clone();
    let retained_db = Arc::new(Mutex::new(None));
    let body_db = retained_db.clone();
    let result = AssertUnwindSafe(with_db_context(
        PartialRuntime::default(),
        move |f, owner| {
            Box::pin(async move {
                *body_db.lock().unwrap() = Some(f.db.clone());
                if cancel {
                    let lock = f.db.begin().await.unwrap();
                    lock.execute_unprepared("LOCK TABLE body_deliveries IN ACCESS EXCLUSIVE MODE")
                        .await
                        .unwrap();
                    owner.lock = Some(lock);
                    {
                        let initialize = Runtime::initialize(&mut owner.runtime, f.clone(), 8);
                        tokio::pin!(initialize);
                        tokio::select! {
                            _ = &mut initialize => panic!("initialization escaped the held SQL lock"),
                            _ = async {
                                loop {
                                    let waiting: bool = f.db.query_one(sea_orm::Statement::from_string(
                                        sea_orm::DatabaseBackend::Postgres,
                                        "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation='body_deliveries'::regclass AND NOT granted) AS waiting",
                                    )).await.unwrap().unwrap().try_get("", "waiting").unwrap();
                                    if waiting { break; }
                                    tokio::time::sleep(Duration::from_millis(1)).await;
                                }
                            } => {}
                        }
                    }
                    // Dropping this actual pending initializer must leave its owner reachable.
                    assert!(owner.runtime.is_some());
                    panic!("injected cancellation at channel initialization SQL");
                } else {
                    f.sql("ALTER TABLE body_deliveries RENAME TO injected_missing_body_deliveries")
                        .await;
                    Runtime::initialize(&mut owner.runtime, f, 8).await;
                    panic!("broken channel SQL unexpectedly initialized");
                }
            })
        },
        move |mut owner| async move {
            if let Some(lock) = owner.lock.take() {
                lock.rollback().await.unwrap();
            }
            let runtime = owner.runtime.take().expect("partial owner was not retained");
            let scene = runtime.scene.clone();
            let path = runtime._directory.0.clone();
            let rpc = runtime.server.task.as_ref().unwrap().abort_handle();
            assert!(!scene.service.shutdown_pending());
            assert!(path.exists());
            runtime.close().await;
            assert!(rpc.is_finished());
            assert!(!scene.service.shutdown_pending());
            assert!(scene.session.active_scope().unwrap().is_none());
            assert!(!path.exists());
            scene.f.db.execute_unprepared("SELECT 1").await.unwrap();
            cleanup_checked.store(true, Ordering::SeqCst);
        },
    ))
    .catch_unwind()
    .await;
    let error = result.expect_err("partial initialization must remain failed");
    let message = error
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| error.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(message.contains(if cancel {
        "injected cancellation at channel initialization SQL"
    } else {
        "relation \"body_deliveries\" does not exist"
    }));
    // The original body panic must not hide a failed cleanup assertion.
    assert!(cleaned.load(Ordering::SeqCst));
    let db = retained_db.lock().unwrap().take().unwrap();
    assert!(db.execute_unprepared("SELECT 1").await.is_err());
}

#[tokio::test]
async fn channel_initialization_sql_error_closes_partial_pair_before_database() {
    partial_initialization(false).await;
}

#[tokio::test]
async fn cancelling_channel_initialization_sql_closes_partial_pair_before_database() {
    partial_initialization(true).await;
}
