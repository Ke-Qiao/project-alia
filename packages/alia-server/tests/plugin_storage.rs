use alia_server::plugins::storage::PluginStorage;
use alia_server::plugins::{
    PluginStorageManifest, PluginStorageRetention, PluginStorageScopeKind, PluginStorageService,
    StorageScope,
};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

#[test]
fn plugin_storage_migration_declares_server_owned_json_store() {
    let sql = include_str!("../migrations/m20260522_000001_create_plugin_storage_records.sql");
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS plugin_storage_records"));
    assert!(sql.contains("value JSONB NOT NULL"));
    assert!(sql.contains("owner_id VARCHAR(200) NOT NULL DEFAULT ''"));
    assert!(sql.contains("UNIQUE (plugin_name, scope, owner_id, collection, key)"));
    assert!(sql.contains("idx_plugin_storage_owner"));
    assert!(sql.contains("idx_plugin_storage_ttl"));
    assert!(sql.contains("scope IN ('global', 'user', 'channel', 'conversation')"));
}

#[test]
fn plugin_storage_manifest_accepts_declared_collections() {
    let manifest: PluginStorageManifest = toml::from_str(
        r#"
        [[collections]]
        name = "settings"
        scope = ["user", "channel"]
        schema_version = 2
        retention = "retain_on_uninstall"

        [[collections]]
        name = "cache"
        scope = ["global"]
        retention = "delete_on_uninstall"
        ttl_sec = 60
        "#,
    )
    .unwrap();

    manifest.validate("weather-bridge").unwrap();
    let settings = manifest.collection("settings").unwrap();
    assert!(settings.allows_scope(PluginStorageScopeKind::User));
    assert_eq!(settings.schema_version, 2);
    assert_eq!(
        settings.retention,
        PluginStorageRetention::RetainOnUninstall
    );
}

#[test]
fn plugin_storage_manifest_rejects_unsafe_declarations() {
    let valid: PluginStorageManifest = toml::from_str(
        r#"
        [[collections]]
        name = "settings"
        scope = ["global"]
        "#,
    )
    .unwrap();
    assert!(valid.validate("component:embedded").is_err());

    let duplicate: PluginStorageManifest = toml::from_str(
        r#"
        [[collections]]
        name = "settings"
        scope = ["global"]

        [[collections]]
        name = "settings"
        scope = ["user"]
        "#,
    )
    .unwrap();
    assert!(duplicate.validate("weather").is_err());

    let export_ttl: PluginStorageManifest = toml::from_str(
        r#"
        [[collections]]
        name = "audit"
        scope = ["global"]
        retention = "export_required"
        ttl_sec = 30
        "#,
    )
    .unwrap();
    assert!(export_ttl.validate("weather").is_err());

    let secrets: PluginStorageManifest = toml::from_str(
        r#"
        [[collections]]
        name = "secrets"
        scope = ["global"]
        "#,
    )
    .unwrap();
    assert!(secrets.validate("weather").is_err());
}

#[tokio::test]
async fn postgres_plugin_storage_round_trips_records_and_policy() {
    let Some(database_url) = std::env::var("ALIA_TEST_DATABASE_URL").ok() else {
        eprintln!(
            "skipping real PostgreSQL plugin storage test: ALIA_TEST_DATABASE_URL is not set"
        );
        return;
    };
    let db = Database::connect(&database_url)
        .await
        .expect("ALIA_TEST_DATABASE_URL should connect");
    apply_plugin_storage_migration(&db).await;

    let plugin_name = format!("test-plugin-{}", Uuid::new_v4());
    let service = PluginStorageService::new(db.clone());
    let manifest: PluginStorageManifest = toml::from_str(
        r#"
        [[collections]]
        name = "settings"
        scope = ["global", "user"]
        schema_version = 2
        retention = "retain_on_uninstall"

        [[collections]]
        name = "cache"
        scope = ["global"]
        retention = "delete_on_uninstall"
        ttl_sec = 60

        [[collections]]
        name = "audit"
        scope = ["global"]
        retention = "export_required"
        "#,
    )
    .unwrap();
    let handle = service
        .scoped_with_manifest(&plugin_name, manifest.clone())
        .unwrap();

    let record = handle
        .put(
            "settings",
            "user/theme",
            StorageScope::User("user-1".to_string()),
            serde_json::json!({"theme": "dark"}),
        )
        .await
        .unwrap();
    assert_eq!(record.version, 1);
    assert_eq!(record.schema_version, 2);

    let updated = handle
        .compare_and_put(
            "settings",
            "user/theme",
            StorageScope::User("user-1".to_string()),
            1,
            serde_json::json!({"theme": "light"}),
        )
        .await
        .unwrap();
    assert_eq!(updated.version, 2);

    let conflict = handle
        .compare_and_put(
            "settings",
            "user/theme",
            StorageScope::User("user-1".to_string()),
            1,
            serde_json::json!({"theme": "blue"}),
        )
        .await
        .unwrap_err();
    assert!(conflict.to_string().contains("version conflict"));

    handle
        .put(
            "settings",
            "user/layout",
            StorageScope::User("user-1".to_string()),
            serde_json::json!({"layout": "compact"}),
        )
        .await
        .unwrap();
    let listed = handle
        .list(
            "settings",
            StorageScope::User("user-1".to_string()),
            Some("user/"),
            10,
        )
        .await
        .unwrap();
    assert_eq!(listed.len(), 2);

    handle
        .put(
            "cache",
            "stale",
            StorageScope::Global,
            serde_json::json!({"ok": true}),
        )
        .await
        .unwrap();
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"
        UPDATE plugin_storage_records
        SET ttl_expires_at = NOW() - INTERVAL '1 second'
        WHERE plugin_name = $1 AND collection = 'cache' AND key = 'stale'
        "#,
        vec![plugin_name.clone().into()],
    ))
    .await
    .unwrap();
    let stale = handle
        .get("cache", "stale", StorageScope::Global)
        .await
        .unwrap();
    assert!(stale.is_none());

    handle
        .put(
            "audit",
            "before-uninstall",
            StorageScope::Global,
            serde_json::json!({"event": "x"}),
        )
        .await
        .unwrap();
    let blocked = service
        .apply_uninstall_policy(&plugin_name, &manifest, false)
        .await
        .unwrap_err();
    assert!(blocked.to_string().contains("requires export"));

    let report = service
        .apply_uninstall_policy(&plugin_name, &manifest, true)
        .await
        .unwrap();
    assert_eq!(report.deleted_records, 1);
    assert_eq!(report.retained_records, 2);
    assert_eq!(report.export_required_records, 1);

    let deleted = service.purge_plugin(&plugin_name, None).await.unwrap();
    assert_eq!(deleted, 3);
}

async fn apply_plugin_storage_migration(db: &DatabaseConnection) {
    db.execute(Statement::from_string(
        DatabaseBackend::Postgres,
        "CREATE EXTENSION IF NOT EXISTS pgcrypto".to_string(),
    ))
    .await
    .unwrap();
    let sql = include_str!("../migrations/m20260522_000001_create_plugin_storage_records.sql");
    for statement in split_sql_statements(sql) {
        db.execute(Statement::from_string(DatabaseBackend::Postgres, statement))
            .await
            .unwrap();
    }
}

fn split_sql_statements(sql: &str) -> Vec<String> {
    sql.lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n")
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .map(|statement| format!("{statement};"))
        .collect()
}
