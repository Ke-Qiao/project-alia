#![cfg(feature = "cache-semantic")]

use alia_server::providers::CacheBackend;
use alia_server::providers::adapter::{CacheEntry, CacheFilter};
use alia_server::providers::cache::two_layer::TwoLayerCache;
use uuid::Uuid;

#[tokio::test]
async fn test_cache_hit() {
    let cache = TwoLayerCache::new();
    let user_id = Some(Uuid::new_v4());

    cache
        .put(CacheEntry {
            user_id,
            scope: "default".to_string(),
            query_text: "What is the weather?".to_string(),
            response_text: "It's sunny!".to_string(),
            metadata: serde_json::json!({}),
            ttl_seconds: 60,
        })
        .await
        .unwrap();

    let result = cache.get(user_id, "What is the weather?").await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().response_text, "It's sunny!");
}

#[tokio::test]
async fn test_cache_miss() {
    let cache = TwoLayerCache::new();
    let result = cache
        .get(Some(Uuid::new_v4()), "Unknown query")
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_invalidate() {
    let cache = TwoLayerCache::new();
    let user_id = Uuid::new_v4();

    for i in 0..5 {
        cache
            .put(CacheEntry {
                user_id: Some(user_id),
                scope: "test".to_string(),
                query_text: format!("Query {}", i),
                response_text: format!("Response {}", i),
                metadata: serde_json::json!({}),
                ttl_seconds: 60,
            })
            .await
            .unwrap();
    }

    let removed = cache
        .invalidate(CacheFilter {
            user_id: Some(user_id),
            scope: Some("test".to_string()),
        })
        .await
        .unwrap();

    assert_eq!(removed, 5);
}
