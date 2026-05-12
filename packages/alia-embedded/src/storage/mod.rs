use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Row;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

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
}
