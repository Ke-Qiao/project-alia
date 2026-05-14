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
    pub keywords: Vec<String>,
    pub audio_path: String,
    pub audio_variants: Vec<DirectResponseAudioRecord>,
    pub action: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectResponseAudioRecord {
    pub audio_path: String,
    pub weight: f32,
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
                keywords_json TEXT NOT NULL,
                audio_path TEXT NOT NULL,
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
            .map(|rule| DirectResponseConfig {
                keywords: rule.keywords,
                audio: rule.audio_path.into(),
                audio_variants: rule
                    .audio_variants
                    .into_iter()
                    .map(|variant| crate::config::DirectResponseAudioConfig {
                        audio: variant.audio_path.into(),
                        weight: variant.weight,
                    })
                    .collect(),
                action: rule.action,
            })
            .collect())
    }

    pub async fn list_direct_responses(
        &self,
        include_disabled: bool,
    ) -> Result<Vec<DirectResponseRuleRecord>> {
        let sql = if include_disabled {
            r#"
            SELECT id, keywords_json, audio_path, audio_variants_json, action, enabled
            FROM direct_responses
            ORDER BY id ASC
            "#
        } else {
            r#"
            SELECT id, keywords_json, audio_path, audio_variants_json, action, enabled
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
        let enabled = i64::from(rule.enabled);

        if rule.id > 0 {
            sqlx::query(
                r#"
                UPDATE direct_responses
                SET keywords_json = ?, audio_path = ?, audio_variants_json = ?, action = ?, enabled = ?, updated_at = CURRENT_TIMESTAMP
                WHERE id = ?
                "#,
            )
            .bind(keywords_json)
            .bind(&rule.audio_path)
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
            INSERT INTO direct_responses (keywords_json, audio_path, audio_variants_json, action, enabled)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(keywords_json)
        .bind(&rule.audio_path)
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
            SELECT id, keywords_json, audio_path, audio_variants_json, action, enabled
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
            INSERT INTO direct_responses (keywords_json, audio_path, audio_variants_json, action)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(keywords_json)
        .bind(response.audio.display().to_string())
        .bind(
            serde_json::to_string(
                &response
                    .audio_variants
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
    let audio_path = row.get::<String, _>("audio_path");
    let variants_json = row
        .try_get::<String, _>("audio_variants_json")
        .unwrap_or_else(|_| "[]".to_string());
    let mut audio_variants: Vec<DirectResponseAudioRecord> =
        serde_json::from_str(&variants_json).unwrap_or_default();
    if audio_variants.is_empty() && !audio_path.trim().is_empty() {
        audio_variants.push(DirectResponseAudioRecord {
            audio_path: audio_path.clone(),
            weight: 1.0,
        });
    }
    Ok(DirectResponseRuleRecord {
        id: row.get::<i64, _>("id"),
        keywords,
        audio_path,
        audio_variants,
        action: row.get::<Option<String>, _>("action"),
        enabled: row.get::<i64, _>("enabled") != 0,
    })
}

fn normalize_audio_variants(rule: &DirectResponseRuleRecord) -> Vec<DirectResponseAudioRecord> {
    let mut variants = rule
        .audio_variants
        .iter()
        .filter(|variant| !variant.audio_path.trim().is_empty())
        .map(|variant| DirectResponseAudioRecord {
            audio_path: variant.audio_path.clone(),
            weight: variant.weight.max(0.0),
        })
        .collect::<Vec<_>>();

    if variants.is_empty() && !rule.audio_path.trim().is_empty() {
        variants.push(DirectResponseAudioRecord {
            audio_path: rule.audio_path.clone(),
            weight: 1.0,
        });
    }

    variants
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
                keywords: vec!["lights on".to_string()],
                audio_path: "assets/responses/light_on.wav".to_string(),
                audio_variants: Vec::new(),
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
}
