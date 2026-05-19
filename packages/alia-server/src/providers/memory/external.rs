//! Adapter layer for externally hosted memory providers.
//!
//! `MemoryBackend` is the server-facing contract. This file keeps provider
//! protocol details behind a smaller client trait so future Zep/Graphiti/plugin
//! clients can be swapped in without touching tools, prompts, or pipelines.

use async_trait::async_trait;
use uuid::Uuid;

use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    MemoryBackend, MemoryCapabilities, MemoryConversationIngest, MemoryListOpts, MemoryNode,
    MemoryPatch, RetrievalOpts,
};

#[async_trait]
pub trait ExternalMemoryClient: Send + Sync {
    fn name(&self) -> &'static str;

    async fn init(&mut self, _ctx: &ModuleContext) -> anyhow::Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }

    async fn store(&self, user_id: Uuid, node: MemoryNode) -> anyhow::Result<String>;
    async fn list(&self, user_id: Uuid, opts: MemoryListOpts) -> anyhow::Result<Vec<MemoryNode>>;
    async fn get(&self, user_id: Uuid, node_id: &str) -> anyhow::Result<Option<MemoryNode>>;
    async fn retrieve(
        &self,
        user_id: Uuid,
        query: &str,
        opts: RetrievalOpts,
    ) -> anyhow::Result<Vec<MemoryNode>>;
    async fn update(&self, node_id: &str, patch: MemoryPatch) -> anyhow::Result<()>;
    async fn delete(&self, node_id: &str) -> anyhow::Result<()>;
    async fn clear(&self, user_id: Uuid) -> anyhow::Result<u64>;

    async fn ingest_conversation(
        &self,
        _user_id: Uuid,
        _ingest: MemoryConversationIngest,
    ) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn memory_capabilities(&self) -> MemoryCapabilities {
        MemoryCapabilities::minimal(self.name())
    }
}

pub struct ExternalMemoryBackend<C> {
    client: C,
}

impl<C> ExternalMemoryBackend<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

#[async_trait]
impl<C> ModuleLifecycle for ExternalMemoryBackend<C>
where
    C: ExternalMemoryClient,
{
    fn name(&self) -> &'static str {
        self.client.name()
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        self.client.init(ctx).await
    }

    async fn health_check(&self) -> ModuleHealth {
        self.client.health_check().await
    }
}

#[async_trait]
impl<C> MemoryBackend for ExternalMemoryBackend<C>
where
    C: ExternalMemoryClient,
{
    async fn store(&self, user_id: Uuid, node: MemoryNode) -> anyhow::Result<String> {
        self.client.store(user_id, node).await
    }

    async fn list(&self, user_id: Uuid, opts: MemoryListOpts) -> anyhow::Result<Vec<MemoryNode>> {
        self.client.list(user_id, opts).await
    }

    async fn get(&self, user_id: Uuid, node_id: &str) -> anyhow::Result<Option<MemoryNode>> {
        self.client.get(user_id, node_id).await
    }

    async fn retrieve(
        &self,
        user_id: Uuid,
        query: &str,
        opts: RetrievalOpts,
    ) -> anyhow::Result<Vec<MemoryNode>> {
        self.client.retrieve(user_id, query, opts).await
    }

    async fn update(&self, node_id: &str, patch: MemoryPatch) -> anyhow::Result<()> {
        self.client.update(node_id, patch).await
    }

    async fn delete(&self, node_id: &str) -> anyhow::Result<()> {
        self.client.delete(node_id).await
    }

    async fn clear(&self, user_id: Uuid) -> anyhow::Result<u64> {
        self.client.clear(user_id).await
    }

    async fn ingest_conversation(
        &self,
        user_id: Uuid,
        ingest: MemoryConversationIngest,
    ) -> anyhow::Result<Option<String>> {
        self.client.ingest_conversation(user_id, ingest).await
    }

    async fn memory_capabilities(&self) -> MemoryCapabilities {
        self.client.memory_capabilities().await
    }
}
