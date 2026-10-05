use std::sync::Arc;

use alia_server::channels::ChannelRegistry;
use alia_server::channels::adapter::{ChannelAdapter, ChannelCapabilities};
use alia_server::module::lifecycle::ModuleLifecycle;
use async_trait::async_trait;

struct TestChannel {
    id: String,
    channel_type: &'static str,
}

impl TestChannel {
    fn new(id: &str, channel_type: &'static str) -> Self {
        Self {
            id: id.to_string(),
            channel_type,
        }
    }
}

#[async_trait]
impl ModuleLifecycle for TestChannel {
    fn name(&self) -> &'static str {
        "test-channel"
    }
}

#[async_trait]
impl ChannelAdapter for TestChannel {
    fn channel_type(&self) -> &'static str {
        self.channel_type
    }

    fn channel_id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities::default()
    }
}

#[tokio::test]
async fn lookup_requires_exact_instance_id_even_for_a_unique_type() {
    let registry = ChannelRegistry::new();
    registry
        .register(Arc::new(TestChannel::new("pi-living-room", "embedded")))
        .await
        .unwrap();
    assert!(registry.get("embedded").await.is_none());
    assert_eq!(
        registry.get("pi-living-room").await.unwrap().channel_id(),
        "pi-living-room"
    );
    registry
        .register(Arc::new(TestChannel::new("pi-office", "embedded")))
        .await
        .unwrap();
    assert!(registry.get("embedded").await.is_none());
    assert_eq!(
        registry.get("pi-office").await.unwrap().channel_id(),
        "pi-office"
    );
}

#[tokio::test]
async fn channel_type_cannot_shadow_a_concrete_id() {
    let registry = ChannelRegistry::new();
    registry
        .register(Arc::new(TestChannel::new("embedded", "cli")))
        .await
        .unwrap();
    registry
        .register(Arc::new(TestChannel::new("pi-office", "embedded")))
        .await
        .unwrap();
    assert_eq!(
        registry.get("embedded").await.unwrap().channel_type(),
        "cli"
    );
    assert_eq!(
        registry.get("pi-office").await.unwrap().channel_type(),
        "embedded"
    );
}

#[tokio::test]
async fn duplicate_active_instance_id_is_rejected() {
    let registry = ChannelRegistry::new();
    registry
        .register(Arc::new(TestChannel::new("pi-office", "embedded")))
        .await
        .unwrap();
    assert!(
        registry
            .register(Arc::new(TestChannel::new("pi-office", "cli")))
            .await
            .is_err()
    );
    assert_eq!(
        registry.get("pi-office").await.unwrap().channel_type(),
        "embedded"
    );
}
