use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::manifest::{PluginStorageManifest, PluginStorageRetention, PluginStorageScopeKind};

const DEFAULT_MAX_RECORD_BYTES: usize = 64 * 1024;
const MAX_LIST_LIMIT: u32 = 500;

#[async_trait]
pub trait PluginStorage: Send + Sync {
    async fn get(
        &self,
        collection: &str,
        key: &str,
        scope: StorageScope,
    ) -> anyhow::Result<Option<PluginRecord>>;

    async fn put(
        &self,
        collection: &str,
        key: &str,
        scope: StorageScope,
        value: serde_json::Value,
    ) -> anyhow::Result<PluginRecord>;

    async fn compare_and_put(
        &self,
        collection: &str,
        key: &str,
        scope: StorageScope,
        expected_version: i64,
        value: serde_json::Value,
    ) -> anyhow::Result<PluginRecord>;

    async fn delete(
        &self,
        collection: &str,
        key: &str,
        scope: StorageScope,
    ) -> anyhow::Result<bool>;

    async fn list(
        &self,
        collection: &str,
        scope: StorageScope,
        prefix: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<Vec<PluginRecord>>;
}

#[derive(Clone)]
pub struct PluginStorageService {
    db: DatabaseConnection,
}

impl PluginStorageService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub fn scoped_with_manifest(
        &self,
        plugin_name: &str,
        manifest: PluginStorageManifest,
    ) -> anyhow::Result<PluginStorageHandle> {
        manifest.validate(plugin_name)?;
        PluginStorageHandle::new(self.db.clone(), plugin_name, Arc::new(manifest))
    }

    pub async fn usage_for_plugin(&self, plugin_name: &str) -> anyhow::Result<PluginStorageUsage> {
        validate_plugin_name(plugin_name)?;
        storage_usage_for_plugin(&self.db, plugin_name, None).await
    }

    pub(crate) async fn usage_for_user(
        &self,
        plugin_name: &str,
        actor: Uuid,
    ) -> anyhow::Result<PluginStorageUsage> {
        validate_plugin_name(plugin_name)?;
        storage_usage_for_plugin(&self.db, plugin_name, Some(actor)).await
    }

    pub async fn list_plugin_usage(&self) -> anyhow::Result<Vec<PluginStorageUsage>> {
        list_plugin_storage_usage(&self.db).await
    }

    pub async fn export_plugin_records(
        &self,
        plugin_name: &str,
    ) -> anyhow::Result<Vec<PluginRecord>> {
        validate_plugin_name(plugin_name)?;
        export_plugin_records(&self.db, plugin_name, None).await
    }

    pub(crate) async fn export_user_records(
        &self,
        plugin_name: &str,
        actor: Uuid,
    ) -> anyhow::Result<Vec<PluginRecord>> {
        validate_plugin_name(plugin_name)?;
        export_plugin_records(&self.db, plugin_name, Some(actor)).await
    }

    pub async fn purge_plugin(
        &self,
        plugin_name: &str,
        collection: Option<&str>,
    ) -> anyhow::Result<u64> {
        validate_plugin_name(plugin_name)?;
        if let Some(collection) = collection {
            validate_collection_name(collection)?;
        }
        purge_plugin_storage(&self.db, plugin_name, collection, None).await
    }

    pub async fn apply_uninstall_policy(
        &self,
        plugin_name: &str,
        manifest: &PluginStorageManifest,
        force_export_required: bool,
    ) -> anyhow::Result<PluginStorageUninstallReport> {
        validate_plugin_name(plugin_name)?;
        manifest.validate(plugin_name)?;
        apply_uninstall_policy(&self.db, plugin_name, manifest, force_export_required).await
    }
}

#[derive(Clone)]
pub struct PluginStorageHandle {
    db: DatabaseConnection,
    plugin_name: String,
    manifest: Arc<PluginStorageManifest>,
}

impl PluginStorageHandle {
    fn new(
        db: DatabaseConnection,
        plugin_name: &str,
        manifest: Arc<PluginStorageManifest>,
    ) -> anyhow::Result<Self> {
        let plugin_name = validate_plugin_name(plugin_name)?;
        Ok(Self {
            db,
            plugin_name,
            manifest,
        })
    }

    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }

    fn collection_policy(
        &self,
        collection: &str,
        scope: StorageScopeKind,
    ) -> anyhow::Result<CollectionPolicy> {
        let collection = validate_collection_name(collection)?;
        let declared = self.manifest.collection(&collection).ok_or_else(|| {
            anyhow::anyhow!(
                "plugin '{}' did not declare storage collection '{}'",
                self.plugin_name,
                collection
            )
        })?;
        if !declared.allows_scope(scope.into()) {
            anyhow::bail!(
                "plugin '{}' collection '{}' does not allow '{}' scope",
                self.plugin_name,
                collection,
                scope.as_str()
            );
        }
        Ok(CollectionPolicy {
            schema_version: declared.schema_version,
            ttl: declared.ttl(),
            max_record_bytes: declared
                .max_record_bytes
                .unwrap_or(DEFAULT_MAX_RECORD_BYTES),
        })
    }

    fn validate_write(
        &self,
        collection: &str,
        key: &str,
        scope: &StorageScope,
        value: &serde_json::Value,
    ) -> anyhow::Result<ResolvedWrite> {
        let scope = scope.validate()?;
        let key = validate_record_key(key)?;
        let policy = self.collection_policy(collection, scope.kind)?;
        let bytes = serde_json::to_vec(value)?.len();
        if bytes > policy.max_record_bytes {
            anyhow::bail!(
                "plugin '{}' storage record '{}:{}' is {} bytes, above {} byte limit",
                self.plugin_name,
                collection,
                key,
                bytes,
                policy.max_record_bytes
            );
        }
        let ttl_expires_at = policy.ttl.map(ttl_expiry).transpose()?;
        Ok(ResolvedWrite {
            collection: validate_collection_name(collection)?,
            key,
            scope,
            schema_version: policy.schema_version,
            ttl_expires_at,
        })
    }

    fn validate_read(
        &self,
        collection: &str,
        key: &str,
        scope: &StorageScope,
    ) -> anyhow::Result<ResolvedRead> {
        let scope = scope.validate()?;
        let key = validate_record_key(key)?;
        self.collection_policy(collection, scope.kind)?;
        Ok(ResolvedRead {
            collection: validate_collection_name(collection)?,
            key,
            scope,
        })
    }
}

#[async_trait]
impl PluginStorage for PluginStorageHandle {
    async fn get(
        &self,
        collection: &str,
        key: &str,
        scope: StorageScope,
    ) -> anyhow::Result<Option<PluginRecord>> {
        let read = self.validate_read(collection, key, &scope)?;
        get_record(&self.db, &self.plugin_name, &read).await
    }

    async fn put(
        &self,
        collection: &str,
        key: &str,
        scope: StorageScope,
        value: serde_json::Value,
    ) -> anyhow::Result<PluginRecord> {
        let write = self.validate_write(collection, key, &scope, &value)?;
        upsert_record(&self.db, &self.plugin_name, &write, value).await
    }

    async fn compare_and_put(
        &self,
        collection: &str,
        key: &str,
        scope: StorageScope,
        expected_version: i64,
        value: serde_json::Value,
    ) -> anyhow::Result<PluginRecord> {
        if expected_version < 0 {
            anyhow::bail!("expected_version must be non-negative");
        }
        let write = self.validate_write(collection, key, &scope, &value)?;
        compare_and_put_record(&self.db, &self.plugin_name, &write, expected_version, value).await
    }

    async fn delete(
        &self,
        collection: &str,
        key: &str,
        scope: StorageScope,
    ) -> anyhow::Result<bool> {
        let read = self.validate_read(collection, key, &scope)?;
        delete_record(&self.db, &self.plugin_name, &read).await
    }

    async fn list(
        &self,
        collection: &str,
        scope: StorageScope,
        prefix: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<Vec<PluginRecord>> {
        let scope = scope.validate()?;
        let collection = validate_collection_name(collection)?;
        self.collection_policy(&collection, scope.kind)?;
        let prefix = prefix
            .map(validate_record_prefix)
            .transpose()?
            .unwrap_or_default();
        list_records(
            &self.db,
            &self.plugin_name,
            &collection,
            scope,
            &prefix,
            limit.clamp(1, MAX_LIST_LIMIT),
        )
        .await
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", content = "owner_id", rename_all = "snake_case")]
pub enum StorageScope {
    Global,
    User(String),
    Channel(String),
    Conversation(String),
}

impl StorageScope {
    pub fn kind(&self) -> StorageScopeKind {
        match self {
            Self::Global => StorageScopeKind::Global,
            Self::User(_) => StorageScopeKind::User,
            Self::Channel(_) => StorageScopeKind::Channel,
            Self::Conversation(_) => StorageScopeKind::Conversation,
        }
    }

    fn validate(&self) -> anyhow::Result<ResolvedScope> {
        let kind = self.kind();
        let owner_id = match self {
            Self::Global => String::new(),
            Self::User(owner) | Self::Channel(owner) | Self::Conversation(owner) => {
                validate_owner_id(owner)?
            }
        };
        Ok(ResolvedScope { kind, owner_id })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageScopeKind {
    Global,
    User,
    Channel,
    Conversation,
}

impl StorageScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::User => "user",
            Self::Channel => "channel",
            Self::Conversation => "conversation",
        }
    }
}

impl From<StorageScopeKind> for PluginStorageScopeKind {
    fn from(value: StorageScopeKind) -> Self {
        match value {
            StorageScopeKind::Global => Self::Global,
            StorageScopeKind::User => Self::User,
            StorageScopeKind::Channel => Self::Channel,
            StorageScopeKind::Conversation => Self::Conversation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginRecord {
    pub id: Uuid,
    pub plugin_name: String,
    pub collection: String,
    pub key: String,
    pub scope: StorageScopeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    pub value: serde_json::Value,
    pub schema_version: i32,
    pub version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginStorageUsage {
    pub plugin_name: String,
    pub record_count: i64,
    pub value_bytes: i64,
    pub expired_record_count: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginStorageUninstallReport {
    pub deleted_records: u64,
    pub retained_records: u64,
    pub export_required_records: u64,
}

#[derive(Debug, Clone)]
struct CollectionPolicy {
    schema_version: i32,
    ttl: Option<Duration>,
    max_record_bytes: usize,
}

#[derive(Debug, Clone)]
struct ResolvedScope {
    kind: StorageScopeKind,
    owner_id: String,
}

#[derive(Debug, Clone)]
struct ResolvedRead {
    collection: String,
    key: String,
    scope: ResolvedScope,
}

#[derive(Debug, Clone)]
struct ResolvedWrite {
    collection: String,
    key: String,
    scope: ResolvedScope,
    schema_version: i32,
    ttl_expires_at: Option<DateTime<Utc>>,
}

pub fn validate_plugin_name(name: &str) -> anyhow::Result<String> {
    validate_identifier(name, "plugin name", 100, false)
}

pub fn validate_collection_name(name: &str) -> anyhow::Result<String> {
    validate_identifier(name, "plugin storage collection", 100, false)
}

fn validate_identifier(
    value: &str,
    label: &str,
    max_len: usize,
    allow_colon: bool,
) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("{label} cannot be empty");
    }
    if value.len() > max_len {
        anyhow::bail!("{label} is too long: {} > {}", value.len(), max_len);
    }
    let valid = value.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || ch == '_'
            || ch == '-'
            || ch == '.'
            || (allow_colon && ch == ':')
    });
    if !valid {
        anyhow::bail!(
            "{label} '{value}' contains unsupported characters; use ASCII letters, numbers, '.', '_' or '-'"
        );
    }
    Ok(value.to_string())
}

fn validate_record_key(key: &str) -> anyhow::Result<String> {
    let key = key.trim();
    if key.is_empty() {
        anyhow::bail!("plugin storage key cannot be empty");
    }
    validate_record_prefix(key)?;
    Ok(key.to_string())
}

fn validate_record_prefix(prefix: &str) -> anyhow::Result<String> {
    if prefix.len() > 200 {
        anyhow::bail!(
            "plugin storage key prefix is too long: {} > 200",
            prefix.len()
        );
    }
    if prefix.chars().any(char::is_control) {
        anyhow::bail!("plugin storage key cannot contain control characters");
    }
    Ok(prefix.to_string())
}

fn validate_owner_id(owner_id: &str) -> anyhow::Result<String> {
    let owner_id = owner_id.trim();
    if owner_id.is_empty() {
        anyhow::bail!("plugin storage owner_id cannot be empty for scoped records");
    }
    if owner_id.len() > 200 {
        anyhow::bail!(
            "plugin storage owner_id is too long: {} > 200",
            owner_id.len()
        );
    }
    if owner_id.chars().any(char::is_control) {
        anyhow::bail!("plugin storage owner_id cannot contain control characters");
    }
    Ok(owner_id.to_string())
}

fn ttl_expiry(ttl: Duration) -> anyhow::Result<DateTime<Utc>> {
    Ok(Utc::now() + ChronoDuration::from_std(ttl).context("ttl_sec is too large")?)
}

fn maybe_ttl_string(value: Option<DateTime<Utc>>) -> String {
    value.map(|dt| dt.to_rfc3339()).unwrap_or_default()
}

async fn get_record(
    db: &DatabaseConnection,
    plugin_name: &str,
    read: &ResolvedRead,
) -> anyhow::Result<Option<PluginRecord>> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT id, plugin_name, collection, key, scope, owner_id, value,
                   schema_version, version, ttl_expires_at, created_at, updated_at
            FROM plugin_storage_records
            WHERE plugin_name = $1
              AND scope = $2
              AND owner_id = $3
              AND collection = $4
              AND key = $5
              AND (ttl_expires_at IS NULL OR ttl_expires_at > NOW())
            "#,
            vec![
                plugin_name.to_string().into(),
                read.scope.kind.as_str().to_string().into(),
                read.scope.owner_id.clone().into(),
                read.collection.clone().into(),
                read.key.clone().into(),
            ],
        ))
        .await?;
    row.as_ref().map(record_from_row).transpose()
}

async fn upsert_record(
    db: &DatabaseConnection,
    plugin_name: &str,
    write: &ResolvedWrite,
    value: serde_json::Value,
) -> anyhow::Result<PluginRecord> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO plugin_storage_records
                (id, plugin_name, collection, key, scope, owner_id, value,
                 schema_version, version, ttl_expires_at, created_at, updated_at)
            VALUES
                ($1::uuid, $2, $3, $4, $5, $6, $7::jsonb,
                 $8, 1, NULLIF($9, '')::timestamptz, NOW(), NOW())
            ON CONFLICT (plugin_name, scope, owner_id, collection, key)
            DO UPDATE SET
                value = EXCLUDED.value,
                schema_version = EXCLUDED.schema_version,
                version = plugin_storage_records.version + 1,
                ttl_expires_at = EXCLUDED.ttl_expires_at,
                updated_at = NOW()
            RETURNING id, plugin_name, collection, key, scope, owner_id, value,
                      schema_version, version, ttl_expires_at, created_at, updated_at
            "#,
            vec![
                Uuid::new_v4().to_string().into(),
                plugin_name.to_string().into(),
                write.collection.clone().into(),
                write.key.clone().into(),
                write.scope.kind.as_str().to_string().into(),
                write.scope.owner_id.clone().into(),
                value.into(),
                write.schema_version.into(),
                maybe_ttl_string(write.ttl_expires_at).into(),
            ],
        ))
        .await?
        .ok_or_else(|| anyhow::anyhow!("plugin storage upsert returned no row"))?;
    record_from_row(&row)
}

async fn compare_and_put_record(
    db: &DatabaseConnection,
    plugin_name: &str,
    write: &ResolvedWrite,
    expected_version: i64,
    value: serde_json::Value,
) -> anyhow::Result<PluginRecord> {
    let row = if expected_version == 0 {
        db.query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO plugin_storage_records
                (id, plugin_name, collection, key, scope, owner_id, value,
                 schema_version, version, ttl_expires_at, created_at, updated_at)
            VALUES
                ($1::uuid, $2, $3, $4, $5, $6, $7::jsonb,
                 $8, 1, NULLIF($9, '')::timestamptz, NOW(), NOW())
            ON CONFLICT (plugin_name, scope, owner_id, collection, key) DO NOTHING
            RETURNING id, plugin_name, collection, key, scope, owner_id, value,
                      schema_version, version, ttl_expires_at, created_at, updated_at
            "#,
            vec![
                Uuid::new_v4().to_string().into(),
                plugin_name.to_string().into(),
                write.collection.clone().into(),
                write.key.clone().into(),
                write.scope.kind.as_str().to_string().into(),
                write.scope.owner_id.clone().into(),
                value.into(),
                write.schema_version.into(),
                maybe_ttl_string(write.ttl_expires_at).into(),
            ],
        ))
        .await?
    } else {
        db.query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            UPDATE plugin_storage_records
            SET value = $7::jsonb,
                schema_version = $8,
                version = version + 1,
                ttl_expires_at = NULLIF($9, '')::timestamptz,
                updated_at = NOW()
            WHERE plugin_name = $1
              AND scope = $2
              AND owner_id = $3
              AND collection = $4
              AND key = $5
              AND version = $6
              AND (ttl_expires_at IS NULL OR ttl_expires_at > NOW())
            RETURNING id, plugin_name, collection, key, scope, owner_id, value,
                      schema_version, version, ttl_expires_at, created_at, updated_at
            "#,
            vec![
                plugin_name.to_string().into(),
                write.scope.kind.as_str().to_string().into(),
                write.scope.owner_id.clone().into(),
                write.collection.clone().into(),
                write.key.clone().into(),
                expected_version.into(),
                value.into(),
                write.schema_version.into(),
                maybe_ttl_string(write.ttl_expires_at).into(),
            ],
        ))
        .await?
    };

    let Some(row) = row else {
        anyhow::bail!(
            "plugin storage version conflict for '{}:{}' in plugin '{}'",
            write.collection,
            write.key,
            plugin_name
        );
    };
    record_from_row(&row)
}

async fn delete_record(
    db: &DatabaseConnection,
    plugin_name: &str,
    read: &ResolvedRead,
) -> anyhow::Result<bool> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            DELETE FROM plugin_storage_records
            WHERE plugin_name = $1
              AND scope = $2
              AND owner_id = $3
              AND collection = $4
              AND key = $5
            "#,
            vec![
                plugin_name.to_string().into(),
                read.scope.kind.as_str().to_string().into(),
                read.scope.owner_id.clone().into(),
                read.collection.clone().into(),
                read.key.clone().into(),
            ],
        ))
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn list_records(
    db: &DatabaseConnection,
    plugin_name: &str,
    collection: &str,
    scope: ResolvedScope,
    prefix: &str,
    limit: u32,
) -> anyhow::Result<Vec<PluginRecord>> {
    let rows = db
        .query_all(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT id, plugin_name, collection, key, scope, owner_id, value,
                   schema_version, version, ttl_expires_at, created_at, updated_at
            FROM plugin_storage_records
            WHERE plugin_name = $1
              AND scope = $2
              AND owner_id = $3
              AND collection = $4
              AND ($5 = '' OR left(key, char_length($5)) = $5)
              AND (ttl_expires_at IS NULL OR ttl_expires_at > NOW())
            ORDER BY key ASC
            LIMIT $6
            "#,
            vec![
                plugin_name.to_string().into(),
                scope.kind.as_str().to_string().into(),
                scope.owner_id.into(),
                collection.to_string().into(),
                prefix.to_string().into(),
                (limit as i64).into(),
            ],
        ))
        .await?;
    rows.iter().map(record_from_row).collect()
}

async fn storage_usage_for_plugin(
    db: &DatabaseConnection,
    plugin_name: &str,
    actor: Option<Uuid>,
) -> anyhow::Result<PluginStorageUsage> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT
                $1::text AS plugin_name,
                COUNT(*)::bigint AS record_count,
                COALESCE(SUM(pg_column_size(value)), 0)::bigint AS value_bytes,
                COUNT(*) FILTER (WHERE ttl_expires_at IS NOT NULL AND ttl_expires_at <= NOW())::bigint
                    AS expired_record_count
            FROM plugin_storage_records
            WHERE plugin_name = $1
              AND ($2::text IS NULL OR (scope = 'user' AND owner_id = $2))
            "#,
            vec![
                plugin_name.to_string().into(),
                actor.map(|id| id.to_string()).into(),
            ],
        ))
        .await?
        .ok_or_else(|| anyhow::anyhow!("plugin storage usage query returned no row"))?;
    usage_from_row(&row)
}

async fn list_plugin_storage_usage(
    db: &DatabaseConnection,
) -> anyhow::Result<Vec<PluginStorageUsage>> {
    let rows = db
        .query_all(Statement::from_string(
            DatabaseBackend::Postgres,
            r#"
            SELECT
                plugin_name,
                COUNT(*)::bigint AS record_count,
                COALESCE(SUM(pg_column_size(value)), 0)::bigint AS value_bytes,
                COUNT(*) FILTER (WHERE ttl_expires_at IS NOT NULL AND ttl_expires_at <= NOW())::bigint
                    AS expired_record_count
            FROM plugin_storage_records
            GROUP BY plugin_name
            ORDER BY plugin_name
            "#
            .to_string(),
        ))
        .await?;
    rows.iter().map(usage_from_row).collect()
}

async fn export_plugin_records(
    db: &DatabaseConnection,
    plugin_name: &str,
    actor: Option<Uuid>,
) -> anyhow::Result<Vec<PluginRecord>> {
    let rows = db
        .query_all(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT id, plugin_name, collection, key, scope, owner_id, value,
                   schema_version, version, ttl_expires_at, created_at, updated_at
            FROM plugin_storage_records
            WHERE plugin_name = $1
              AND ($2::text IS NULL OR (scope = 'user' AND owner_id = $2))
              AND (ttl_expires_at IS NULL OR ttl_expires_at > NOW())
            ORDER BY collection, scope, owner_id, key
            "#,
            vec![
                plugin_name.to_string().into(),
                actor.map(|id| id.to_string()).into(),
            ],
        ))
        .await?;
    rows.iter().map(record_from_row).collect()
}

pub(crate) async fn purge_plugin_storage(
    db: &impl ConnectionTrait,
    plugin_name: &str,
    collection: Option<&str>,
    actor: Option<Uuid>,
) -> anyhow::Result<u64> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM plugin_storage_records WHERE plugin_name = $1
             AND ($2::text IS NULL OR collection = $2)
             AND ($3::text IS NULL OR (scope = 'user' AND owner_id = $3))",
            vec![
                plugin_name.to_string().into(),
                collection.map(str::to_owned).into(),
                actor.map(|id| id.to_string()).into(),
            ],
        ))
        .await?;
    Ok(result.rows_affected())
}

async fn apply_uninstall_policy(
    db: &DatabaseConnection,
    plugin_name: &str,
    manifest: &PluginStorageManifest,
    force_export_required: bool,
) -> anyhow::Result<PluginStorageUninstallReport> {
    let mut report = PluginStorageUninstallReport::default();

    if !force_export_required {
        for collection in &manifest.collections {
            if collection.retention != PluginStorageRetention::ExportRequired {
                continue;
            }
            let count = count_collection_records(db, plugin_name, &collection.name).await?;
            if count > 0 {
                anyhow::bail!(
                    "plugin '{}' collection '{}' requires export or explicit purge before uninstall",
                    plugin_name,
                    collection.name
                );
            }
        }
    }

    for collection in &manifest.collections {
        let count = count_collection_records(db, plugin_name, &collection.name).await?;
        match collection.retention {
            PluginStorageRetention::DeleteOnUninstall => {
                report.deleted_records +=
                    purge_plugin_storage(db, plugin_name, Some(&collection.name), None).await?;
            }
            PluginStorageRetention::RetainOnUninstall => {
                report.retained_records += count;
            }
            PluginStorageRetention::ExportRequired => {
                report.export_required_records += count;
            }
        }
    }

    Ok(report)
}

async fn count_collection_records(
    db: &DatabaseConnection,
    plugin_name: &str,
    collection: &str,
) -> anyhow::Result<u64> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT COUNT(*)::bigint AS count
            FROM plugin_storage_records
            WHERE plugin_name = $1 AND collection = $2
            "#,
            vec![
                plugin_name.to_string().into(),
                collection.to_string().into(),
            ],
        ))
        .await?
        .ok_or_else(|| anyhow::anyhow!("plugin storage count query returned no row"))?;
    let count: i64 = row.try_get("", "count")?;
    Ok(count.max(0) as u64)
}

fn record_from_row(row: &QueryResult) -> anyhow::Result<PluginRecord> {
    let scope: String = row.try_get("", "scope")?;
    let scope = match scope.as_str() {
        "global" => StorageScopeKind::Global,
        "user" => StorageScopeKind::User,
        "channel" => StorageScopeKind::Channel,
        "conversation" => StorageScopeKind::Conversation,
        other => anyhow::bail!("unknown plugin storage scope '{other}'"),
    };
    let owner_id: String = row.try_get("", "owner_id")?;
    let ttl_expires_at: Option<DateTimeWithTimeZone> = row.try_get("", "ttl_expires_at")?;
    let created_at: DateTimeWithTimeZone = row.try_get("", "created_at")?;
    let updated_at: DateTimeWithTimeZone = row.try_get("", "updated_at")?;

    Ok(PluginRecord {
        id: row.try_get("", "id")?,
        plugin_name: row.try_get("", "plugin_name")?,
        collection: row.try_get("", "collection")?,
        key: row.try_get("", "key")?,
        scope,
        owner_id: if owner_id.is_empty() {
            None
        } else {
            Some(owner_id)
        },
        value: row.try_get("", "value")?,
        schema_version: row.try_get("", "schema_version")?,
        version: row.try_get("", "version")?,
        ttl_expires_at: ttl_expires_at.map(|dt| dt.with_timezone(&Utc)),
        created_at: created_at.with_timezone(&Utc),
        updated_at: updated_at.with_timezone(&Utc),
    })
}

fn usage_from_row(row: &QueryResult) -> anyhow::Result<PluginStorageUsage> {
    Ok(PluginStorageUsage {
        plugin_name: row.try_get("", "plugin_name")?,
        record_count: row.try_get("", "record_count")?,
        value_bytes: row.try_get("", "value_bytes")?,
        expired_record_count: row.try_get("", "expired_record_count")?,
    })
}
