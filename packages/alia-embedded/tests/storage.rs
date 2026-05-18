use std::path::PathBuf;
use std::str::FromStr;

use alia_embedded::storage::{DirectResponseAudioRecord, DirectResponseRuleRecord, EmbeddedStore};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

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
        .upsert_direct_response(&DirectResponseRuleRecord {
            id: 0,
            slug: String::new(),
            keywords: vec!["现在几点".to_string()],
            audio_variants: vec![DirectResponseAudioRecord {
                audio_path: "assets/responses/time.wav".to_string(),
                weight: 1.0,
            }],
            action: Some("report_time".to_string()),
            enabled: true,
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

    let store = EmbeddedStore::open(&path).await.unwrap();
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display())).unwrap();
    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .unwrap();
    let columns = sqlx::query("PRAGMA table_info(direct_responses)")
        .fetch_all(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<Vec<_>>();
    assert!(!columns.iter().any(|column| column == "audio_path"));
    pool.close().await;

    let rules = store.list_direct_responses(true).await.unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].audio_variants.len(), 1);
    assert_eq!(rules[0].audio_variants[0].audio_path, "legacy.wav");
    assert_eq!(rules[0].audio_variants[0].weight, 1.0);
}
