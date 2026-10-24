//! Mandatory PostgreSQL integration tests. Set ALIA_TEST_DATABASE_URL to a
//! disposable PostgreSQL database with pgvector available; missing infrastructure
//! is a test failure, never a passing early return.

use alia_server::modules::platform::migrations::{self, MIGRATIONS, Migration};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement,
};
use uuid::Uuid;

#[test]
fn bundled_manifest_contains_every_sql_migration() {
    let mut files = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    files.sort();
    let bundled = MIGRATIONS
        .iter()
        .map(|migration| migration.version)
        .collect::<Vec<_>>();
    assert_eq!(
        files, bundled,
        "every migration must ship in the executable"
    );
}

struct TestDatabase {
    admin: DatabaseConnection,
    db: DatabaseConnection,
    schema: String,
}

impl TestDatabase {
    async fn new() -> Self {
        let url = std::env::var("ALIA_TEST_DATABASE_URL")
            .expect("set ALIA_TEST_DATABASE_URL to run real PostgreSQL migration tests");
        let admin = Database::connect(&url)
            .await
            .expect("test PostgreSQL must be reachable");
        assert_eq!(admin.get_database_backend(), DatabaseBackend::Postgres);
        let schema = format!("migration_test_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
            .await
            .unwrap();
        let mut options = ConnectOptions::new(url);
        options
            .max_connections(4)
            .min_connections(1)
            .set_schema_search_path(format!("{schema},public"))
            .sqlx_logging(false);
        let db = Database::connect(options).await.unwrap();
        Self { admin, db, schema }
    }

    async fn cleanup(self) {
        self.db.close().await.unwrap();
        self.admin
            .execute_unprepared(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .await
            .unwrap();
        self.admin.close().await.unwrap();
    }

    async fn table_exists(&self, table: &str) -> bool {
        self.db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT to_regclass($1) IS NOT NULL AS present",
                [format!("{}.{}", self.schema, table).into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "present")
            .unwrap()
    }

    async fn journal_count(&self) -> i64 {
        self.db
            .query_one(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT COUNT(*) AS count FROM schema_migrations",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "count")
            .unwrap()
    }
}

#[tokio::test]
async fn motion_rename_preserves_historical_json_and_unknown_without_replay() {
    use alia_core::embodiment::motion::{MotionCommand, MotionProgress};
    use serde_json::{Value, json};

    let database = TestDatabase::new().await;
    let old = *MIGRATIONS
        .iter()
        .find(|m| m.version == "m20260913_000006_body_delivery.sql")
        .unwrap();
    let rename = *MIGRATIONS
        .iter()
        .find(|m| m.version == "m20261019_000001_rename_body_delivery.sql")
        .unwrap();
    let prerequisites = Migration {
        version: "000_motion_fixture",
        sql: "CREATE TABLE delivery_responses(response_id UUID PRIMARY KEY); CREATE TABLE delivery_events(response_id UUID NOT NULL, kind TEXT NOT NULL, payload JSONB NOT NULL);",
    };
    migrations::apply(&database.db, &[prerequisites, old])
        .await
        .unwrap();
    let response_id = Uuid::new_v4();
    let delivery_id = Uuid::new_v4();
    let dispatch_id = Uuid::new_v4();
    let boot = Uuid::new_v4();
    // These are the existing persisted JSON shapes: Rust type/module names do
    // not appear in either command or receipt evidence.
    let command_json = json!({
        "identity": {"lease": {"device_boot_id": boot, "server_instance_id": Uuid::new_v4(), "generation": 1}, "sequence": 1},
        "plan_id": Uuid::new_v4(), "source_command_id": "historical-command", "owner_id": Uuid::new_v4(),
        "scope": {"kind": "nonvoice", "scope_id": Uuid::new_v4()}, "priority": "normal", "interruptible": true,
        "actions": [{"duration_ms": 400, "intensity": 0.5, "motion": {"kind": "small_nod"}}]
    });
    let checkpoint_json = json!({
        "identity": command_json["identity"], "receipt_sequence": 1,
        "accepted_actions": 1, "started_actions": 1, "submitted_actions": 1,
        "submitted_frames": 3, "partial_frame": null, "last_partial_channels": [], "last_uncertain_channels": [],
        "backend": {"instance_id": boot, "boundary": "simulated", "channels": 3},
        "writer_quiescent": true, "position_confirmed_actions": null, "end": {"kind": "submitted"}
    });
    let command: MotionCommand = serde_json::from_value(command_json.clone()).unwrap();
    command.validate().unwrap();
    let checkpoint: MotionProgress = serde_json::from_value(checkpoint_json.clone()).unwrap();
    checkpoint.validate(None, &command).unwrap();
    database
        .db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO delivery_responses VALUES($1)",
            [response_id.into()],
        ))
        .await
        .unwrap();
    database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "INSERT INTO body_deliveries(delivery_id,response_id,output_key,channel_id,source_command_id,dispatch_id,identity,command,status,unknown_reason,lease_expires_at,observation_deadline_at) VALUES($1,$2,'speech_start','device','historical-command',$3,$4,$5,'unknown','original ACK lost',clock_timestamp(),clock_timestamp())",
        vec![delivery_id.into(), response_id.into(), dispatch_id.into(), command_json["identity"].clone().into(), command_json.clone().into()])).await.unwrap();
    database.db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "INSERT INTO body_delivery_late_checkpoints(delivery_id,receipt_sequence,progress) VALUES($1,1,$2)",
        vec![delivery_id.into(), checkpoint_json.clone().into()])).await.unwrap();
    database
        .db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO delivery_events VALUES($1,'body_dispatch',$2)",
            vec![
                response_id.into(),
                json!({"body_delivery_id": delivery_id, "command": command_json}).into(),
            ],
        ))
        .await
        .unwrap();

    let report = migrations::apply(&database.db, &[prerequisites, old, rename])
        .await
        .unwrap();
    assert_eq!((report.applied, report.unchanged), (1, 2));
    assert!(!database.table_exists("body_deliveries").await);
    assert!(
        !database
            .table_exists("body_delivery_late_checkpoints")
            .await
    );
    let row = database.db.query_one(Statement::from_string(DatabaseBackend::Postgres,
        "SELECT d.status,d.dispatch_id,d.command,c.progress FROM motion_deliveries d JOIN motion_delivery_late_checkpoints c USING(delivery_id)")).await.unwrap().unwrap();
    assert_eq!(row.try_get::<String>("", "status").unwrap(), "unknown");
    assert_eq!(row.try_get::<Uuid>("", "dispatch_id").unwrap(), dispatch_id);
    assert_eq!(row.try_get::<Value>("", "command").unwrap(), command_json);
    assert_eq!(
        row.try_get::<Value>("", "progress").unwrap(),
        checkpoint_json
    );
    let event = database
        .db
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT kind,payload FROM delivery_events",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event.try_get::<String>("", "kind").unwrap(),
        "motion_dispatch"
    );
    assert_eq!(
        event.try_get::<Value>("", "payload").unwrap(),
        json!({"motion_delivery_id": delivery_id, "command": command_json})
    );
    let residual = database.db.query_one(Statement::from_string(DatabaseBackend::Postgres,
        "SELECT count(*) AS count FROM pg_constraint WHERE conrelid IN ('motion_deliveries'::regclass, 'motion_delivery_late_checkpoints'::regclass) AND conname LIKE 'body_%'")).await.unwrap().unwrap();
    assert_eq!(residual.try_get::<i64>("", "count").unwrap(), 0);
    assert!(!database.table_exists("body_deliveries_expiry").await);
    assert!(database.table_exists("motion_deliveries_expiry").await);
    let repeated = migrations::apply(&database.db, &[prerequisites, old, rename])
        .await
        .unwrap();
    assert_eq!((repeated.applied, repeated.unchanged), (0, 3));
    database.cleanup().await;
}

#[tokio::test]
async fn failed_migration_rolls_back_sql_and_journal_then_recovers() {
    let database = TestDatabase::new().await;
    let initial = Migration {
        version: "001_initial",
        sql: "CREATE TABLE retained (id INTEGER PRIMARY KEY); INSERT INTO retained VALUES (1);",
    };
    let broken = Migration {
        version: "002_recoverable",
        sql: "CREATE TABLE partial_write (value TEXT); INSERT INTO retained VALUES (2); SELECT 1 / 0;",
    };
    let error = migrations::apply(&database.db, &[initial, broken])
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("002_recoverable"));
    assert!(format!("{error:#}").contains("division by zero"));
    assert!(database.table_exists("retained").await);
    assert!(!database.table_exists("partial_write").await);
    assert_eq!(database.journal_count().await, 1);
    let count: i64 = database
        .db
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT COUNT(*) AS count FROM retained",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "count")
        .unwrap();
    assert_eq!(
        count, 1,
        "data writes in the failed migration must roll back"
    );

    let repaired = Migration {
        version: broken.version,
        sql: "CREATE TABLE partial_write (value TEXT); INSERT INTO retained VALUES (2);",
    };
    let report = migrations::apply(&database.db, &[initial, repaired])
        .await
        .unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(report.unchanged, 1);
    assert!(database.table_exists("partial_write").await);
    let repeated = migrations::apply(&database.db, &[initial, repaired])
        .await
        .unwrap();
    assert_eq!(repeated.applied, 0);
    assert_eq!(repeated.unchanged, 2);
    assert_eq!(database.journal_count().await, 2);
    database.cleanup().await;
}

#[tokio::test]
async fn first_migration_failure_rolls_back_journal_creation() {
    let database = TestDatabase::new().await;
    let error = migrations::apply(
        &database.db,
        &[Migration {
            version: "001_broken",
            sql: "CREATE TABLE partial_write (id INTEGER); SELECT 1 / 0;",
        }],
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("001_broken"));
    assert!(!database.table_exists("partial_write").await);
    assert!(!database.table_exists("schema_migrations").await);
    database.cleanup().await;
}

#[tokio::test]
async fn concurrent_startups_apply_each_migration_once() {
    let database = TestDatabase::new().await;
    let migrations = [Migration {
        version: "001_concurrent",
        sql: "CREATE TABLE executions (id INTEGER); SELECT pg_sleep(0.15); INSERT INTO executions VALUES (1);",
    }];
    let (first, second) = tokio::join!(
        migrations::apply(&database.db, &migrations),
        migrations::apply(&database.db, &migrations),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.applied + second.applied, 1);
    assert_eq!(first.unchanged + second.unchanged, 1);
    assert_eq!(database.journal_count().await, 1);
    let count: i64 = database
        .db
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT COUNT(*) AS count FROM executions",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "count")
        .unwrap();
    assert_eq!(count, 1);
    database.cleanup().await;
}

#[tokio::test]
async fn changed_applied_sql_is_rejected_without_executing_it() {
    let database = TestDatabase::new().await;
    migrations::apply(
        &database.db,
        &[Migration {
            version: "001_immutable",
            sql: "CREATE TABLE original (id INTEGER);",
        }],
    )
    .await
    .unwrap();
    let error = migrations::apply(
        &database.db,
        &[Migration {
            version: "001_immutable",
            sql: "CREATE TABLE unexpected_change (id INTEGER);",
        }],
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("checksum mismatch"));
    assert!(database.table_exists("original").await);
    assert!(!database.table_exists("unexpected_change").await);
    assert_eq!(database.journal_count().await, 1);
    database.cleanup().await;
}

#[tokio::test]
async fn sql_scripts_preserve_quoted_semicolons_and_function_bodies() {
    let database = TestDatabase::new().await;
    migrations::apply(&database.db, &[Migration {
        version: "001_script",
        sql: "CREATE TABLE messages (body TEXT); DO $$ BEGIN INSERT INTO messages VALUES ('quoted; semicolon'); END $$;",
    }]).await.unwrap();
    let body: String = database
        .db
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT body FROM messages",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "body")
        .unwrap();
    assert_eq!(body, "quoted; semicolon");
    database.cleanup().await;
}

#[tokio::test]
async fn bundled_schema_installs_with_pgvector_and_restarts_without_changes() {
    let database = TestDatabase::new().await;
    let first = migrations::run(&database.db).await.unwrap();
    let extension = database.admin.query_one(Statement::from_string(DatabaseBackend::Postgres,
        "SELECT n.nspname AS schema_name FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='vector'"))
        .await.unwrap().unwrap();
    assert_eq!(
        extension.try_get::<String>("", "schema_name").unwrap(),
        "public",
        "application schema cleanup must not own the shared pgvector extension"
    );
    assert_eq!(first.applied, MIGRATIONS.len());
    assert!(database.table_exists("memory_items").await);
    assert!(database.table_exists("memory_embeddings").await);
    assert!(!database.table_exists("memory_embeddings_1536").await);
    assert!(database.table_exists("plugin_storage_records").await);
    let repeated = migrations::run(&database.db).await.unwrap();
    assert_eq!(repeated.applied, 0);
    assert_eq!(repeated.unchanged, MIGRATIONS.len());
    assert_eq!(database.journal_count().await, MIGRATIONS.len() as i64);
    database.cleanup().await;
}
