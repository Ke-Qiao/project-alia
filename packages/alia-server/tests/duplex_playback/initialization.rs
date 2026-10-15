//! Failed fixture construction must remove its own schema before returning.
use super::delivery_support::{InitializationStep, probe_initialization};
use futures::FutureExt;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use std::{
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
};

async fn failed_initialization(step: InitializationStep) {
    let mut options = ConnectOptions::new(std::env::var("ALIA_TEST_DATABASE_URL").unwrap());
    options.max_connections(1).sqlx_logging(false);
    let observer = Database::connect(options).await.unwrap();
    let evidence = Arc::new(Mutex::new(None));
    let observed = evidence.clone();
    let result = AssertUnwindSafe(probe_initialization(move |current, fixture| {
        let observed = observed.clone();
        Box::pin(async move {
            if current != step {
                return;
            }
            *observed.lock().unwrap() =
                Some((fixture.schema_name().to_string(), fixture.db.clone()));
            match step {
                InitializationStep::CreateSchema => unreachable!("failure probe starts after CREATE"),
                InitializationStep::WorkPool => panic!("injected work pool initialization failure"),
                InitializationStep::UserSeed => {
                    fixture.db.execute_unprepared(
                        "ALTER TABLE users RENAME COLUMN password_hash TO injected_missing_password_hash",
                    ).await.unwrap();
                }
                InitializationStep::Policy => {
                    fixture.db.execute_unprepared(
                        "ALTER TABLE context_policy RENAME COLUMN policy TO injected_missing_policy",
                    ).await.unwrap();
                }
            }
        })
    }))
    .catch_unwind()
    .await;
    let (schema, work_pool) = evidence.lock().unwrap().take().unwrap();
    let schema_remains: bool = observer
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname=$1) AS present",
            [schema.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "present")
        .unwrap();
    let work_pool_remains = work_pool.execute_unprepared("SELECT 1").await.is_ok();
    // Capture the failure first, then reclaim the old implementation's resources.
    if !matches!(work_pool, DatabaseConnection::Disconnected) {
        work_pool.close().await.unwrap();
    }
    assert!(
        schema
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    );
    observer
        .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .await
        .unwrap();
    observer.close().await.unwrap();
    let error = result.expect_err("initialization failure must not become success");
    let text = error
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| error.downcast_ref::<&str>().copied())
        .unwrap();
    let expected = match step {
        InitializationStep::CreateSchema => unreachable!("failure probe starts after CREATE"),
        InitializationStep::WorkPool => "injected work pool initialization failure",
        InitializationStep::UserSeed => "password_hash",
        InitializationStep::Policy => "policy",
    };
    assert!(
        text.contains(expected),
        "original failure was not preserved"
    );
    assert!(
        !schema_remains,
        "failed initialization left its schema behind"
    );
    assert!(
        !work_pool_remains,
        "failed initialization left its work pool open"
    );
}

#[tokio::test]
async fn work_pool_initialization_panic_removes_created_schema() {
    failed_initialization(InitializationStep::WorkPool).await;
}

#[tokio::test]
async fn user_seed_sql_failure_removes_schema_and_closes_work_pool() {
    failed_initialization(InitializationStep::UserSeed).await;
}

#[tokio::test]
async fn policy_sql_failure_removes_schema_and_closes_work_pool() {
    failed_initialization(InitializationStep::Policy).await;
}
