//! PostgreSQL migrations packaged with the executable.
//!
//! Each migration and its journal entry commit together. A transaction-scoped
//! advisory lock serializes concurrent startup attempts, including journal
//! creation; PostgreSQL releases it after commit, rollback, or connection loss.

use anyhow::{Context, ensure};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, Statement,
    TransactionTrait,
};
use sha2::{Digest, Sha256};

// Stable across server versions; scoped to the PostgreSQL database.
const MIGRATION_LOCK_KEY: i64 = 0x414c_4941_4d49_4752;

#[derive(Debug, Clone, Copy)]
pub struct Migration<'a> {
    pub version: &'a str,
    pub sql: &'a str,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub applied: usize,
    pub unchanged: usize,
}

macro_rules! migration {
    ($name:literal) => {
        Migration {
            version: $name,
            sql: include_str!(concat!("../../../migrations/", $name)),
        }
    };
}

pub const MIGRATIONS: &[Migration<'static>] = &[
    migration!("m20260507_000001_enable_pgvector.sql"),
    migration!("m20260507_000004_create_llm_cache.sql"),
    migration!("m20260507_000005_create_skills_tools.sql"),
    migration!("m20260508_000001_create_personas.sql"),
    migration!("m20260509_000001_create_insight_logs.sql"),
    migration!("m20260509_000002_create_sensors.sql"),
    migration!("m20260511_000001_create_chat_sessions.sql"),
    migration!("m20260516_000001_normalize_skill_tools.sql"),
    migration!("m20260516_000002_create_registry_artifacts.sql"),
    migration!("m20260517_000001_add_tool_catalog_metadata.sql"),
    migration!("m20260518_000001_create_conversation_events.sql"),
    migration!("m20260518_000002_create_automation.sql"),
    migration!("m20260518_000003_create_users_auth.sql"),
    migration!("m20260519_000001_create_expression_appraisal_events.sql"),
    migration!("m20260520_000001_create_memory_runtime.sql"),
    migration!("m20260522_000001_create_plugin_storage_records.sql"),
    migration!("m20260908_000001_create_execution_ledger.sql"),
    migration!("m20260908_000002_memory_privacy.sql"),
    migration!("m20260908_000003_source_privacy_epoch.sql"),
    migration!("m20260908_000004_delivery.sql"),
    migration!("m20260908_000005_text_delivery.sql"),
    migration!("m20260913_000006_body_delivery.sql"),
    migration!("m20260913_000007_context_lease.sql"),
    migration!("m20260913_000008_cache_context.sql"),
    migration!("m20260913_000009_memory_embedding_identity.sql"),
    migration!("m20260913_000010_appraisal_provenance.sql"),
    migration!("m20260914_000011_appraisal_rebuild.sql"),
    migration!("m20260920_000012_appraisal_memory_erasure.sql"),
    migration!("m20260920_000013_output_control.sql"),
    migration!("m20260926_000014_companion.sql"),
];

pub async fn run(db: &DatabaseConnection) -> anyhow::Result<MigrationReport> {
    ensure!(
        db.get_database_backend() == DatabaseBackend::Postgres,
        "server migrations require PostgreSQL"
    );
    // Extensions belong to the database, not the caller's application schema.
    // An isolated schema must never acquire ownership of a globally shared
    // vector type that its later removal could cascade into other schemas.
    let tx = db.begin().await.context("preparing database extensions")?;
    tx.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock($1)",
        [MIGRATION_LOCK_KEY.into()],
    ))
    .await?;
    tx.execute_unprepared("CREATE EXTENSION IF NOT EXISTS vector WITH SCHEMA public")
        .await
        .context("installing pgvector in the shared public schema")?;
    let extension = tx.query_one(Statement::from_string(DatabaseBackend::Postgres,
        "SELECT n.nspname AS schema_name FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='vector'"))
        .await?.context("pgvector was not installed")?;
    ensure!(
        extension.try_get::<String>("", "schema_name")? == "public",
        "pgvector must be installed in public; configure the database extension schema before starting Alia"
    );
    tx.commit()
        .await
        .context("committing database extension setup")?;
    apply(db, MIGRATIONS).await
}

/// Apply a complete, ordered set of trusted application migrations.
///
/// SQL must be transaction-safe and must not contain transaction-control
/// commands (BEGIN/COMMIT/ROLLBACK). Execute whole scripts through PostgreSQL's
/// SQL parser so quoted semicolons, comments, and function bodies remain intact.
/// Applied migrations are immutable: change behavior in a new migration.
pub async fn apply(
    db: &DatabaseConnection,
    migrations: &[Migration<'_>],
) -> anyhow::Result<MigrationReport> {
    ensure!(
        db.get_database_backend() == DatabaseBackend::Postgres,
        "server migrations require PostgreSQL"
    );
    for (index, migration) in migrations.iter().enumerate() {
        ensure!(
            !migration.version.trim().is_empty() && !migration.sql.trim().is_empty(),
            "migration #{index} must have a version and SQL"
        );
        if let Some(previous) = index.checked_sub(1).map(|i| &migrations[i]) {
            ensure!(
                previous.version < migration.version,
                "migrations must have unique ascending versions: {} then {}",
                previous.version,
                migration.version
            );
        }
    }

    let mut report = MigrationReport::default();
    for migration in migrations {
        let transaction = db
            .begin()
            .await
            .with_context(|| format!("starting migration {}", migration.version))?;
        let result = apply_in_transaction(&transaction, migration).await;
        let applied = match result {
            Ok(applied) => {
                transaction
                    .commit()
                    .await
                    .with_context(|| format!("committing migration {}", migration.version))?;
                applied
            }
            Err(error) => {
                if let Err(rollback_error) = transaction.rollback().await {
                    return Err(error.context(format!(
                        "rollback also failed for migration {}: {rollback_error}",
                        migration.version
                    )));
                }
                return Err(error);
            }
        };
        if applied {
            report.applied += 1;
            tracing::info!(migration = migration.version, "applied SQL migration");
        } else {
            report.unchanged += 1;
        }
    }
    Ok(report)
}

async fn apply_in_transaction(
    transaction: &DatabaseTransaction,
    migration: &Migration<'_>,
) -> anyhow::Result<bool> {
    transaction
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [MIGRATION_LOCK_KEY.into()],
        ))
        .await
        .with_context(|| format!("locking database for migration {}", migration.version))?;
    transaction
        .execute_unprepared(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version TEXT PRIMARY KEY,
                checksum TEXT NOT NULL,
                applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )",
        )
        .await
        .context("ensuring migration journal")?;

    let checksum = format!("{:x}", Sha256::digest(migration.sql.as_bytes()));
    if let Some(record) = transaction
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT checksum FROM schema_migrations WHERE version = $1",
            [migration.version.into()],
        ))
        .await
        .with_context(|| format!("reading journal for migration {}", migration.version))?
    {
        let recorded: String = record.try_get("", "checksum")?;
        ensure!(
            recorded == checksum,
            "checksum mismatch for applied migration {}; add a new migration instead of editing it",
            migration.version
        );
        return Ok(false);
    }

    transaction
        .execute_unprepared(migration.sql)
        .await
        .with_context(|| format!("applying SQL migration {}", migration.version))?;
    transaction
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO schema_migrations (version, checksum) VALUES ($1, $2)",
            [migration.version.into(), checksum.into()],
        ))
        .await
        .with_context(|| format!("recording migration {}", migration.version))?;
    Ok(true)
}
