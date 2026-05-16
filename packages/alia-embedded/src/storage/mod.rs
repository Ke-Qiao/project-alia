use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::Row;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

use crate::config::DirectResponseConfig;

#[derive(Debug, Clone, PartialEq)]
pub struct DirectResponseRuleRecord {
    pub id: i64,
    pub slug: String,
    pub keywords: Vec<String>,
    pub audio_variants: Vec<DirectResponseAudioRecord>,
    pub action: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectResponseAudioRecord {
    pub audio_path: String,
    pub weight: f32,
}

#[derive(Debug, Clone, Deserialize)]
struct DirectResponseImportConfig {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    audio: Option<PathBuf>,
    #[serde(default)]
    audio_path: Option<PathBuf>,
    #[serde(default)]
    audio_variants: Vec<DirectResponseImportAudio>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default = "default_enabled")]
    enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct DirectResponseImportAudio {
    #[serde(default)]
    audio: Option<PathBuf>,
    #[serde(default)]
    audio_path: Option<PathBuf>,
    #[serde(default = "default_weight")]
    weight: f32,
}

#[derive(Clone)]
pub struct EmbeddedStore {
    pool: SqlitePool,
    path: PathBuf,
}

impl EmbeddedStore {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("creating SQLite state directory: {}", parent.display())
            })?;
        }

        let connection_string = format!("sqlite://{}", path.display());
        let options = SqliteConnectOptions::from_str(&connection_string)
            .with_context(|| format!("building SQLite connection options for {}", path.display()))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .with_context(|| format!("opening SQLite store at {}", path.display()))?;

        let store = Self { pool, path };
        store.bootstrap().await?;
        Ok(store)
    }

    async fn bootstrap(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS embedded_state (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .context("ensuring embedded_state table")?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS direct_responses (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                slug TEXT NOT NULL DEFAULT '',
                keywords_json TEXT NOT NULL,
                audio_variants_json TEXT NOT NULL DEFAULT '[]',
                action TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .context("ensuring direct_responses table")?;
        self.ensure_direct_response_columns().await?;
        self.migrate_direct_responses_drop_audio_path().await?;
        self.ensure_direct_response_indexes().await?;
        self.assign_missing_direct_response_slugs().await?;

        Ok(())
    }

    async fn ensure_direct_response_columns(&self) -> Result<()> {
        let rows = sqlx::query("PRAGMA table_info(direct_responses)")
            .fetch_all(&self.pool)
            .await
            .context("reading direct_responses table info")?;
        let has_audio_variants = rows
            .iter()
            .any(|row| row.get::<String, _>("name") == "audio_variants_json");
        let has_slug = rows
            .iter()
            .any(|row| row.get::<String, _>("name") == "slug");

        if !has_audio_variants {
            sqlx::query(
                r#"
                ALTER TABLE direct_responses
                ADD COLUMN audio_variants_json TEXT NOT NULL DEFAULT '[]'
                "#,
            )
            .execute(&self.pool)
            .await
            .context("adding direct_responses.audio_variants_json")?;
        }
        if !has_slug {
            sqlx::query(
                r#"
                ALTER TABLE direct_responses
                ADD COLUMN slug TEXT NOT NULL DEFAULT ''
                "#,
            )
            .execute(&self.pool)
            .await
            .context("adding direct_responses.slug")?;
        }

        Ok(())
    }

    async fn migrate_direct_responses_drop_audio_path(&self) -> Result<()> {
        let rows = sqlx::query("PRAGMA table_info(direct_responses)")
            .fetch_all(&self.pool)
            .await
            .context("reading direct_responses table info before migration")?;
        let has_audio_path = rows
            .iter()
            .any(|row| row.get::<String, _>("name") == "audio_path");
        if !has_audio_path {
            return Ok(());
        }

        let rows = sqlx::query(
            r#"
            SELECT id, slug, keywords_json, audio_path, audio_variants_json, action, enabled, updated_at
            FROM direct_responses
            ORDER BY id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("loading legacy direct responses for audio_path migration")?;

        let mut tx = self
            .pool
            .begin()
            .await
            .context("starting direct_responses migration")?;

        sqlx::query(
            r#"
            CREATE TABLE direct_responses_new (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                slug TEXT NOT NULL DEFAULT '',
                keywords_json TEXT NOT NULL,
                audio_variants_json TEXT NOT NULL DEFAULT '[]',
                action TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&mut *tx)
        .await
        .context("creating direct_responses_new")?;

        for row in rows {
            let audio_path = row.get::<String, _>("audio_path");
            let variants_json = row
                .try_get::<String, _>("audio_variants_json")
                .unwrap_or_else(|_| "[]".to_string());
            let mut variants: Vec<DirectResponseAudioRecord> =
                serde_json::from_str(&variants_json).unwrap_or_default();
            if variants.is_empty() && !audio_path.trim().is_empty() {
                variants.push(DirectResponseAudioRecord {
                    audio_path,
                    weight: 1.0,
                });
            }
            let variants_json =
                serde_json::to_string(&variants).context("serializing migrated audio variants")?;

            sqlx::query(
                r#"
                INSERT INTO direct_responses_new
                    (id, slug, keywords_json, audio_variants_json, action, enabled, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(row.get::<i64, _>("id"))
            .bind(row.try_get::<String, _>("slug").unwrap_or_default())
            .bind(row.get::<String, _>("keywords_json"))
            .bind(variants_json)
            .bind(row.get::<Option<String>, _>("action"))
            .bind(row.get::<i64, _>("enabled"))
            .bind(row.get::<String, _>("updated_at"))
            .execute(&mut *tx)
            .await
            .context("copying direct response into migrated table")?;
        }

        sqlx::query("DROP TABLE direct_responses")
            .execute(&mut *tx)
            .await
            .context("dropping legacy direct_responses table")?;
        sqlx::query("ALTER TABLE direct_responses_new RENAME TO direct_responses")
            .execute(&mut *tx)
            .await
            .context("renaming migrated direct_responses table")?;

        tx.commit()
            .await
            .context("committing direct_responses audio_path migration")?;
        tracing::info!("migrated direct_responses table to remove audio_path column");
        Ok(())
    }

    async fn ensure_direct_response_indexes(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE UNIQUE INDEX IF NOT EXISTS idx_direct_responses_slug
            ON direct_responses(slug)
            WHERE slug <> ''
            "#,
        )
        .execute(&self.pool)
        .await
        .context("ensuring direct_responses slug index")?;
        Ok(())
    }

    async fn assign_missing_direct_response_slugs(&self) -> Result<()> {
        let rows = sqlx::query("SELECT id, slug, keywords_json, action FROM direct_responses")
            .fetch_all(&self.pool)
            .await
            .context("loading direct response slugs")?;

        let mut used = HashSet::new();
        for row in &rows {
            let slug = row.get::<String, _>("slug");
            if !slug.trim().is_empty() {
                used.insert(slug);
            }
        }

        for row in rows {
            let id = row.get::<i64, _>("id");
            let slug = row.get::<String, _>("slug");
            if !slug.trim().is_empty() {
                continue;
            }
            let keywords_json = row.get::<String, _>("keywords_json");
            let keywords: Vec<String> = serde_json::from_str(&keywords_json).unwrap_or_default();
            let action = row.get::<Option<String>, _>("action");
            let desired = action
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| keywords.first().map(String::as_str))
                .unwrap_or("direct-response");
            let slug = unique_slug(desired, &mut used);

            sqlx::query("UPDATE direct_responses SET slug = ? WHERE id = ?")
                .bind(&slug)
                .bind(id)
                .execute(&self.pool)
                .await
                .with_context(|| format!("assigning slug '{slug}' to direct response {id}"))?;
        }

        Ok(())
    }

    pub async fn load_json<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        let row = sqlx::query("SELECT value_json FROM embedded_state WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .with_context(|| format!("loading state key '{key}' from {}", self.path.display()))?;

        let Some(row) = row else {
            return Ok(None);
        };

        let payload = row.get::<String, _>("value_json");
        let value = serde_json::from_str(&payload)
            .with_context(|| format!("deserializing state key '{key}' from SQLite"))?;
        Ok(Some(value))
    }

    pub async fn save_json<T>(&self, key: &str, value: &T) -> Result<()>
    where
        T: Serialize,
    {
        let payload = serde_json::to_string(value)
            .with_context(|| format!("serializing state key '{key}' for SQLite"))?;

        sqlx::query(
            r#"
            INSERT INTO embedded_state (key, value_json, updated_at)
            VALUES (?, ?, CURRENT_TIMESTAMP)
            ON CONFLICT(key) DO UPDATE SET
                value_json = excluded.value_json,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(key)
        .bind(payload)
        .execute(&self.pool)
        .await
        .with_context(|| format!("saving state key '{key}' to {}", self.path.display()))?;

        Ok(())
    }

    pub async fn load_direct_responses(&self) -> Result<Vec<DirectResponseConfig>> {
        Ok(self
            .list_direct_responses(false)
            .await?
            .into_iter()
            .map(|rule| {
                let audio = first_audio_path(&rule).into();
                DirectResponseConfig {
                    slug: rule.slug,
                    keywords: rule.keywords,
                    audio,
                    audio_variants: rule
                        .audio_variants
                        .into_iter()
                        .map(|variant| crate::config::DirectResponseAudioConfig {
                            audio: variant.audio_path.into(),
                            weight: variant.weight,
                        })
                        .collect(),
                    action: rule.action,
                }
            })
            .collect())
    }

    pub async fn list_direct_responses(
        &self,
        include_disabled: bool,
    ) -> Result<Vec<DirectResponseRuleRecord>> {
        let sql = if include_disabled {
            r#"
            SELECT id, slug, keywords_json, audio_variants_json, action, enabled
            FROM direct_responses
            ORDER BY id ASC
            "#
        } else {
            r#"
            SELECT id, slug, keywords_json, audio_variants_json, action, enabled
            FROM direct_responses
            WHERE enabled = 1
            ORDER BY id ASC
            "#
        };

        let rows = sqlx::query(sql)
            .fetch_all(&self.pool)
            .await
            .with_context(|| format!("loading direct responses from {}", self.path.display()))?;

        rows.into_iter()
            .map(row_to_direct_response_record)
            .collect()
    }

    pub async fn upsert_direct_response(
        &self,
        rule: &DirectResponseRuleRecord,
    ) -> Result<DirectResponseRuleRecord> {
        let keywords_json =
            serde_json::to_string(&rule.keywords).context("serializing keywords")?;
        let audio_variants = normalize_audio_variants(rule);
        let audio_variants_json =
            serde_json::to_string(&audio_variants).context("serializing audio variants")?;
        let slug = if sanitize_slug(&rule.slug).is_empty() {
            self.next_direct_response_slug(rule).await?
        } else {
            sanitize_slug(&rule.slug)
        };
        let enabled = i64::from(rule.enabled);

        if rule.id > 0 {
            sqlx::query(
                r#"
                UPDATE direct_responses
                SET slug = ?, keywords_json = ?, audio_variants_json = ?, action = ?, enabled = ?, updated_at = CURRENT_TIMESTAMP
                WHERE id = ?
                "#,
            )
            .bind(&slug)
            .bind(keywords_json)
            .bind(&audio_variants_json)
            .bind(&rule.action)
            .bind(enabled)
            .bind(rule.id)
            .execute(&self.pool)
            .await
            .with_context(|| format!("updating direct response {}", rule.id))?;

            return self
                .get_direct_response(rule.id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("direct response {} not found", rule.id));
        }

        let result = sqlx::query(
            r#"
            INSERT INTO direct_responses (slug, keywords_json, audio_variants_json, action, enabled)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(&slug)
        .bind(keywords_json)
        .bind(&audio_variants_json)
        .bind(&rule.action)
        .bind(enabled)
        .execute(&self.pool)
        .await
        .context("inserting direct response")?;

        let id = result.last_insert_rowid();
        self.get_direct_response(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("inserted direct response {id} not found"))
    }

    pub async fn direct_response_slug_exists(&self, slug: &str) -> Result<bool> {
        let slug = sanitize_slug(slug);
        if slug.is_empty() {
            return Ok(false);
        }

        let exists = sqlx::query("SELECT 1 FROM direct_responses WHERE slug = ? LIMIT 1")
            .bind(slug)
            .fetch_optional(&self.pool)
            .await
            .context("checking direct response slug")?
            .is_some();
        Ok(exists)
    }

    async fn next_direct_response_slug(&self, rule: &DirectResponseRuleRecord) -> Result<String> {
        let rows = sqlx::query("SELECT slug FROM direct_responses WHERE slug <> ''")
            .fetch_all(&self.pool)
            .await
            .context("loading direct response slugs")?;
        let mut used = rows
            .into_iter()
            .map(|row| row.get::<String, _>("slug"))
            .collect::<HashSet<_>>();
        let desired = rule
            .action
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| rule.keywords.first().map(String::as_str))
            .unwrap_or("direct-response");
        Ok(unique_slug(desired, &mut used))
    }

    pub async fn import_direct_response_bundles(&self, root: &Path) -> Result<usize> {
        let roots = direct_response_import_roots(root);
        let mut seen = HashSet::new();
        let mut imported = 0usize;

        for root in roots {
            if !root.exists() {
                tracing::debug!(
                    root = %root.display(),
                    "direct response import root does not exist; skipping"
                );
                continue;
            }

            let configs = find_direct_response_configs(&root).with_context(|| {
                format!("scanning direct response import root {}", root.display())
            })?;

            for config_path in configs {
                match self
                    .import_direct_response_bundle(&root, &config_path, &mut seen)
                    .await
                {
                    Ok(true) => imported += 1,
                    Ok(false) => {}
                    Err(error) => {
                        tracing::warn!(
                            config = %config_path.display(),
                            error = ?error,
                            "failed to import direct response config; leaving config.json in place"
                        );
                    }
                }
            }
        }

        if imported > 0 {
            tracing::info!(
                requested_root = %root.display(),
                imported,
                "imported direct response bundles into SQLite"
            );
        }

        Ok(imported)
    }

    async fn import_direct_response_bundle(
        &self,
        _root: &Path,
        config_path: &Path,
        seen: &mut HashSet<String>,
    ) -> Result<bool> {
        let config_dir = config_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("direct response config has no parent directory"))?;
        let content = std::fs::read_to_string(config_path)
            .with_context(|| format!("reading {}", config_path.display()))?;
        let import: DirectResponseImportConfig = serde_json::from_str(&content)
            .with_context(|| format!("parsing {}", config_path.display()))?;

        let slug = slug_from_import(&import, config_dir)?;
        if !seen.insert(slug.clone()) {
            tracing::warn!(
                slug,
                config = %config_path.display(),
                "duplicate direct response slug in import scan; skipping"
            );
            return Ok(false);
        }
        let existing = self.get_direct_response_by_slug(&slug).await?;

        let mut audio_variants = import_audio_variants(&import, config_dir)?;
        if audio_variants.is_empty() {
            audio_variants = discover_wav_variants(config_dir)?;
        }
        let keywords = import
            .keywords
            .into_iter()
            .filter(|keyword| !keyword.trim().is_empty())
            .collect::<Vec<_>>();
        let action = import.action.filter(|action| !action.trim().is_empty());
        if keywords.is_empty() && action.is_none() && audio_variants.is_empty() {
            anyhow::bail!("direct response import requires a keyword, action, or audio variant");
        }

        let id = existing
            .as_ref()
            .map(|record| record.id)
            .unwrap_or_default();
        self.upsert_direct_response(&DirectResponseRuleRecord {
            id,
            slug: slug.clone(),
            keywords,
            audio_variants,
            action,
            enabled: import.enabled,
        })
        .await?;

        match std::fs::remove_file(config_path) {
            Ok(()) => tracing::info!(
                slug,
                updated = existing.is_some(),
                config = %config_path.display(),
                "imported direct response config into SQLite and removed config.json"
            ),
            Err(error) => tracing::warn!(
                slug,
                config = %config_path.display(),
                error = ?error,
                "imported direct response config but failed to remove config.json"
            ),
        }

        Ok(true)
    }

    async fn get_direct_response_by_slug(
        &self,
        slug: &str,
    ) -> Result<Option<DirectResponseRuleRecord>> {
        let row = sqlx::query(
            r#"
            SELECT id, slug, keywords_json, audio_variants_json, action, enabled
            FROM direct_responses
            WHERE slug = ?
            "#,
        )
        .bind(slug)
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("loading direct response slug {slug}"))?;

        row.map(row_to_direct_response_record).transpose()
    }

    pub async fn delete_direct_response(&self, id: i64) -> Result<bool> {
        let result = sqlx::query("DELETE FROM direct_responses WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .with_context(|| format!("deleting direct response {id}"))?;
        Ok(result.rows_affected() > 0)
    }

    async fn get_direct_response(&self, id: i64) -> Result<Option<DirectResponseRuleRecord>> {
        let row = sqlx::query(
            r#"
            SELECT id, slug, keywords_json, audio_variants_json, action, enabled
            FROM direct_responses
            WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .with_context(|| format!("loading direct response {id}"))?;

        row.map(row_to_direct_response_record).transpose()
    }

    #[cfg(test)]
    pub async fn insert_direct_response(&self, response: &DirectResponseConfig) -> Result<()> {
        let keywords_json =
            serde_json::to_string(&response.keywords).context("serializing keywords")?;
        sqlx::query(
            r#"
            INSERT INTO direct_responses (keywords_json, audio_variants_json, action)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(keywords_json)
        .bind(
            serde_json::to_string(
                &audio_variants_from_config(response)
                    .iter()
                    .map(|variant| DirectResponseAudioRecord {
                        audio_path: variant.audio.display().to_string(),
                        weight: variant.weight,
                    })
                    .collect::<Vec<_>>(),
            )
            .context("serializing test audio variants")?,
        )
        .bind(response.action.clone())
        .execute(&self.pool)
        .await
        .context("inserting direct response")?;
        Ok(())
    }
}

fn row_to_direct_response_record(row: sqlx::sqlite::SqliteRow) -> Result<DirectResponseRuleRecord> {
    let keywords_json = row.get::<String, _>("keywords_json");
    let keywords =
        serde_json::from_str(&keywords_json).context("deserializing direct response keywords")?;
    let variants_json = row
        .try_get::<String, _>("audio_variants_json")
        .unwrap_or_else(|_| "[]".to_string());
    let audio_variants: Vec<DirectResponseAudioRecord> =
        serde_json::from_str(&variants_json).unwrap_or_default();
    Ok(DirectResponseRuleRecord {
        id: row.get::<i64, _>("id"),
        slug: row.try_get::<String, _>("slug").unwrap_or_default(),
        keywords,
        audio_variants,
        action: row.get::<Option<String>, _>("action"),
        enabled: row.get::<i64, _>("enabled") != 0,
    })
}

fn find_direct_response_configs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut configs = Vec::new();
    collect_direct_response_configs(root, &mut configs)?;
    configs.sort();
    Ok(configs)
}

fn direct_response_import_roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_unique_path(&mut roots, root.to_path_buf());

    let Some(parent) = root.parent() else {
        return roots;
    };

    for name in [
        "direct_response",
        "direct_responses",
        "direct-response",
        "direct-responses",
    ] {
        push_unique_path(&mut roots, parent.join(name));
    }

    roots
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn collect_direct_response_configs(dir: &Path, configs: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading entry under {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading file type for {}", path.display()))?;

        if file_type.is_dir() {
            collect_direct_response_configs(&path, configs)?;
        } else if file_type.is_file() && path.file_name() == Some(OsStr::new("config.json")) {
            configs.push(path);
        }
    }
    Ok(())
}

fn slug_from_import(import: &DirectResponseImportConfig, config_dir: &Path) -> Result<String> {
    let candidate = if import.slug.trim().is_empty() {
        config_dir
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_string()
    } else {
        import.slug.clone()
    };
    let slug = sanitize_slug(&candidate);
    if slug.is_empty() {
        anyhow::bail!("direct response import slug is empty");
    }
    Ok(slug)
}

fn import_audio_variants(
    import: &DirectResponseImportConfig,
    config_dir: &Path,
) -> Result<Vec<DirectResponseAudioRecord>> {
    let mut variants = import
        .audio_variants
        .iter()
        .map(|variant| {
            let path = variant
                .audio_path
                .as_ref()
                .or(variant.audio.as_ref())
                .ok_or_else(|| anyhow::anyhow!("audio variant requires audio_path or audio"))?;
            let resolved = resolve_bundle_path(config_dir, path);
            if !resolved.is_file() {
                anyhow::bail!(
                    "direct response audio file does not exist: {}",
                    resolved.display()
                );
            }
            Ok(DirectResponseAudioRecord {
                audio_path: resolved.display().to_string(),
                weight: variant.weight.max(0.0),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if variants.is_empty()
        && let Some(path) = import.audio_path.as_ref().or(import.audio.as_ref())
    {
        let resolved = resolve_bundle_path(config_dir, path);
        if !resolved.is_file() {
            anyhow::bail!(
                "direct response audio file does not exist: {}",
                resolved.display()
            );
        }
        variants.push(DirectResponseAudioRecord {
            audio_path: resolved.display().to_string(),
            weight: 1.0,
        });
    }

    Ok(variants)
}

fn discover_wav_variants(config_dir: &Path) -> Result<Vec<DirectResponseAudioRecord>> {
    let mut variants = Vec::new();
    for entry in std::fs::read_dir(config_dir)
        .with_context(|| format!("reading {}", config_dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("reading entry under {}", config_dir.display()))?;
        let path = entry.path();
        if !entry
            .file_type()
            .with_context(|| format!("reading file type for {}", path.display()))?
            .is_file()
        {
            continue;
        }
        let is_wav = path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"));
        if is_wav {
            variants.push(DirectResponseAudioRecord {
                audio_path: path.display().to_string(),
                weight: 1.0,
            });
        }
    }
    variants.sort_by(|left, right| left.audio_path.cmp(&right.audio_path));
    Ok(variants)
}

fn resolve_bundle_path(config_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_dir.join(path)
    }
}

fn sanitize_slug(input: &str) -> String {
    input
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn unique_slug(input: &str, used: &mut HashSet<String>) -> String {
    let base = {
        let slug = sanitize_slug(input);
        if slug.is_empty() {
            "direct-response".to_string()
        } else {
            slug
        }
    };
    if used.insert(base.clone()) {
        return base;
    }

    for index in 2.. {
        let candidate = format!("{base}-{index}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }

    unreachable!("unbounded slug generation exhausted")
}

fn default_enabled() -> bool {
    true
}

fn default_weight() -> f32 {
    1.0
}

fn normalize_audio_variants(rule: &DirectResponseRuleRecord) -> Vec<DirectResponseAudioRecord> {
    rule.audio_variants
        .iter()
        .filter(|variant| !variant.audio_path.trim().is_empty())
        .map(|variant| DirectResponseAudioRecord {
            audio_path: variant.audio_path.clone(),
            weight: variant.weight.max(0.0),
        })
        .collect::<Vec<_>>()
}

fn first_audio_path(rule: &DirectResponseRuleRecord) -> String {
    rule.audio_variants
        .first()
        .map(|variant| variant.audio_path.clone())
        .unwrap_or_default()
}

#[cfg(test)]
fn audio_variants_from_config(
    response: &DirectResponseConfig,
) -> Vec<crate::config::DirectResponseAudioConfig> {
    if response.audio_variants.is_empty() && response.audio.as_os_str().is_empty() {
        return Vec::new();
    }
    if response.audio_variants.is_empty() {
        return vec![crate::config::DirectResponseAudioConfig {
            audio: response.audio.clone(),
            weight: 1.0,
        }];
    }
    response.audio_variants.clone()
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct ExampleState {
        value: String,
    }

    #[tokio::test]
    async fn round_trips_json_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embedded-state.sqlite3");
        let store = EmbeddedStore::open(&path).await.unwrap();

        let expected = ExampleState {
            value: "hello".to_string(),
        };
        store.save_json("example", &expected).await.unwrap();

        let actual = store.load_json::<ExampleState>("example").await.unwrap();
        assert_eq!(actual, Some(expected));
    }

    #[tokio::test]
    async fn loads_direct_response_rules() {
        let dir = tempfile::tempdir().unwrap();
        let store = EmbeddedStore::open(dir.path().join("embedded-state.sqlite3"))
            .await
            .unwrap();
        store
            .insert_direct_response(&DirectResponseConfig {
                slug: String::new(),
                keywords: vec!["现在几点".to_string()],
                audio: "assets/responses/time.wav".into(),
                audio_variants: Vec::new(),
                action: Some("report_time".to_string()),
            })
            .await
            .unwrap();

        let responses = store.load_direct_responses().await.unwrap();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].keywords, vec!["现在几点"]);
        assert_eq!(
            responses[0].audio,
            PathBuf::from("assets/responses/time.wav")
        );
        assert_eq!(responses[0].action.as_deref(), Some("report_time"));
    }

    #[tokio::test]
    async fn upserts_and_deletes_direct_response_rules() {
        let dir = tempfile::tempdir().unwrap();
        let store = EmbeddedStore::open(dir.path().join("embedded-state.sqlite3"))
            .await
            .unwrap();

        let inserted = store
            .upsert_direct_response(&DirectResponseRuleRecord {
                id: 0,
                slug: String::new(),
                keywords: vec!["lights on".to_string()],
                audio_variants: vec![DirectResponseAudioRecord {
                    audio_path: "assets/responses/light_on.wav".to_string(),
                    weight: 1.0,
                }],
                action: Some("gpio_light_on".to_string()),
                enabled: true,
            })
            .await
            .unwrap();
        assert!(inserted.id > 0);

        let mut updated = inserted.clone();
        updated.enabled = false;
        let updated = store.upsert_direct_response(&updated).await.unwrap();
        assert!(!updated.enabled);

        let all = store.list_direct_responses(true).await.unwrap();
        assert_eq!(all.len(), 1);
        let enabled = store.list_direct_responses(false).await.unwrap();
        assert!(enabled.is_empty());

        assert!(store.delete_direct_response(inserted.id).await.unwrap());
        assert!(!store.delete_direct_response(inserted.id).await.unwrap());
    }

    #[tokio::test]
    async fn upserts_action_only_direct_response_rule() {
        let dir = tempfile::tempdir().unwrap();
        let store = EmbeddedStore::open(dir.path().join("embedded-state.sqlite3"))
            .await
            .unwrap();

        let inserted = store
            .upsert_direct_response(&DirectResponseRuleRecord {
                id: 0,
                slug: "cancel".to_string(),
                keywords: vec!["退下".to_string()],
                audio_variants: Vec::new(),
                action: Some("cancel".to_string()),
                enabled: true,
            })
            .await
            .unwrap();

        assert_eq!(inserted.slug, "cancel");
        assert_eq!(inserted.audio_variants, Vec::new());
        let rules = store.load_direct_responses().await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].audio_variants.len(), 0);
        assert_eq!(rules[0].action.as_deref(), Some("cancel"));
    }

    #[tokio::test]
    async fn imports_direct_response_bundle_and_removes_config() {
        let dir = tempfile::tempdir().unwrap();
        let store = EmbeddedStore::open(dir.path().join("embedded-state.sqlite3"))
            .await
            .unwrap();
        let bundle = dir.path().join("direct_response").join("wakeword");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("hello.wav"), b"RIFF....WAVE").unwrap();
        std::fs::write(
            bundle.join("config.json"),
            r#"{
                "slug": "wakeword",
                "keywords": ["hey alia", "你好阿利亚"],
                "action": "wakeword",
                "audio_variants": [
                    { "audio": "hello.wav", "weight": 1.0 }
                ]
            }"#,
        )
        .unwrap();

        let imported = store
            .import_direct_response_bundles(&dir.path().join("direct_response"))
            .await
            .unwrap();
        assert_eq!(imported, 1);
        assert!(!bundle.join("config.json").exists());

        let rules = store.list_direct_responses(true).await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].slug, "wakeword");
        assert_eq!(rules[0].action.as_deref(), Some("wakeword"));
        assert_eq!(rules[0].audio_variants.len(), 1);
        assert_eq!(rules[0].audio_variants[0].weight, 1.0);
        assert_eq!(
            rules[0].audio_variants[0].audio_path,
            bundle.join("hello.wav").display().to_string()
        );
    }

    #[tokio::test]
    async fn imports_action_only_direct_response_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let store = EmbeddedStore::open(dir.path().join("embedded-state.sqlite3"))
            .await
            .unwrap();
        let bundle = dir.path().join("direct_response").join("cancel");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(
            bundle.join("config.json"),
            r#"{
                "slug": "cancel",
                "keywords": ["退下", "拜拜"],
                "action": "cancel"
            }"#,
        )
        .unwrap();

        let imported = store
            .import_direct_response_bundles(&dir.path().join("direct_response"))
            .await
            .unwrap();
        assert_eq!(imported, 1);
        assert!(!bundle.join("config.json").exists());

        let rules = store.list_direct_responses(true).await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].slug, "cancel");
        assert_eq!(rules[0].audio_variants, Vec::new());
        assert_eq!(rules[0].action.as_deref(), Some("cancel"));
    }

    #[tokio::test]
    async fn updates_existing_direct_response_slug_and_removes_config() {
        let dir = tempfile::tempdir().unwrap();
        let store = EmbeddedStore::open(dir.path().join("embedded-state.sqlite3"))
            .await
            .unwrap();
        let existing = store
            .upsert_direct_response(&DirectResponseRuleRecord {
                id: 0,
                slug: "wakeword".to_string(),
                keywords: vec!["hey alia".to_string()],
                audio_variants: vec![DirectResponseAudioRecord {
                    audio_path: "wakeword/hello.wav".to_string(),
                    weight: 1.0,
                }],
                action: Some("wakeword".to_string()),
                enabled: true,
            })
            .await
            .unwrap();

        let bundle = dir.path().join("direct_response").join("wakeword");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("hello.wav"), b"RIFF....WAVE").unwrap();
        std::fs::write(
            bundle.join("config.json"),
            r#"{
                "slug": "wakeword",
                "keywords": ["hey alia"],
                "audio_variants": [{ "audio": "hello.wav" }]
            }"#,
        )
        .unwrap();

        let imported = store
            .import_direct_response_bundles(&dir.path().join("direct_response"))
            .await
            .unwrap();
        assert_eq!(imported, 1);
        assert!(!bundle.join("config.json").exists());

        let rules = store.list_direct_responses(true).await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, existing.id);
        assert_eq!(rules[0].slug, "wakeword");
        assert_eq!(rules[0].keywords, vec!["hey alia"]);
        assert_eq!(
            rules[0].audio_variants[0].audio_path,
            bundle.join("hello.wav").display().to_string()
        );
    }

    #[tokio::test]
    async fn scans_direct_response_alias_roots() {
        let dir = tempfile::tempdir().unwrap();
        let store = EmbeddedStore::open(dir.path().join("embedded-state.sqlite3"))
            .await
            .unwrap();
        let bundle = dir.path().join("direct-responses").join("wakeword");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("hello.wav"), b"RIFF....WAVE").unwrap();
        std::fs::write(
            bundle.join("config.json"),
            r#"{
                "slug": "wakeword",
                "keywords": ["hey alia"],
                "audio_variants": [{ "audio": "hello.wav" }]
            }"#,
        )
        .unwrap();

        let imported = store
            .import_direct_response_bundles(&dir.path().join("direct_response"))
            .await
            .unwrap();

        assert_eq!(imported, 1);
        assert!(!bundle.join("config.json").exists());
        let rules = store.list_direct_responses(true).await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].slug, "wakeword");
    }

    #[tokio::test]
    async fn migrates_legacy_audio_path_column_into_variants() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embedded-state.sqlite3");
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE direct_responses (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                slug TEXT NOT NULL DEFAULT '',
                keywords_json TEXT NOT NULL,
                audio_path TEXT NOT NULL,
                audio_variants_json TEXT NOT NULL DEFAULT '[]',
                action TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO direct_responses
                (slug, keywords_json, audio_path, audio_variants_json, action, enabled)
            VALUES ('legacy', '["legacy"]', 'legacy.wav', '[]', 'legacy_action', 1)
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        let store = EmbeddedStore::open(path).await.unwrap();
        let columns = sqlx::query("PRAGMA table_info(direct_responses)")
            .fetch_all(&store.pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect::<Vec<_>>();
        assert!(!columns.iter().any(|column| column == "audio_path"));

        let rules = store.list_direct_responses(true).await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].audio_variants.len(), 1);
        assert_eq!(rules[0].audio_variants[0].audio_path, "legacy.wav");
        assert_eq!(rules[0].audio_variants[0].weight, 1.0);
    }
}
