use std::sync::Arc;

use alia_server::channels::ChannelRegistry;
use alia_server::channels::adapter::{ChannelAdapter, ChannelCapabilities, ChannelTarget};
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

    async fn send_message(&self, _target: &ChannelTarget, _msg: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn resolves_unique_channel_type_alias_to_concrete_id() {
    let registry = ChannelRegistry::new();
    registry
        .register(Arc::new(TestChannel::new("pi-living-room", "embedded")))
        .await
        .unwrap();

    let resolved = registry.resolve_id_or_type("embedded").await.unwrap();
    assert_eq!(resolved.as_deref(), Some("pi-living-room"));
}

#[tokio::test]
async fn rejects_channel_id_that_conflicts_with_existing_channel_type() {
    let registry = ChannelRegistry::new();
    registry
        .register(Arc::new(TestChannel::new("pi-living-room", "embedded")))
        .await
        .unwrap();

    let error = registry
        .register(Arc::new(TestChannel::new("embedded", "cli")))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("conflicts with existing channel type")
    );
}

#[tokio::test]
async fn rejects_channel_type_that_conflicts_with_existing_channel_id() {
    let registry = ChannelRegistry::new();
    registry
        .register(Arc::new(TestChannel::new("embedded", "embedded")))
        .await
        .unwrap();

    let error = registry
        .register(Arc::new(TestChannel::new("custom-one", "embedded")))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("conflicts with existing channel id")
    );
}

#[tokio::test]
async fn ambiguous_channel_type_alias_fails() {
    let registry = ChannelRegistry::new();
    registry
        .register(Arc::new(TestChannel::new("pi-living-room", "embedded")))
        .await
        .unwrap();
    registry
        .register(Arc::new(TestChannel::new("pi-office", "embedded")))
        .await
        .unwrap();

    let error = registry.resolve_id_or_type("embedded").await.unwrap_err();
    assert!(error.to_string().contains("matched multiple channels"));
}
