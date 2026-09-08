//! Built-in Alia Memory Runtime.
//!
//! Memory is a core runtime, not a replaceable provider slot. The production
//! path persists canonical memory in PostgreSQL, stores embeddings in a profile
//! specific side table, and projects lightweight graph edges for retrieval.
//! Explicit tests can use the in-memory repository; configured persistence never falls back.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};

use super::privacy;
use uuid::Uuid;

use crate::config::{MemorySection, default_memory_active_embedding_profile};
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    EmbeddingAdapter, GraphFragment, GraphSnippet, MemoryBackend, MemoryCapabilities,
    MemoryConsolidationResult, MemoryContextPack, MemoryEdge, MemoryExport, MemoryListOpts,
    MemoryNode, MemoryPatch, MemoryWriteContext, RetrievalOpts, SuppressedMemory,
    memory_context_pack_from_nodes,
};

const DEFAULT_TENANT_ID: Uuid = Uuid::nil();
const EMBEDDING_PROFILE_1536: &str = "openai.text-embedding-3-small.1536.cosine.v1";

type UserMemories = HashMap<Uuid, Vec<MemoryNode>>;

#[derive(Debug, Clone)]
struct MemoryRuntimeConfig {
    active_embedding_profile: String,
    graph_depth: u32,
    consolidation_enabled: bool,
}

impl Default for MemoryRuntimeConfig {
    fn default() -> Self {
        Self {
            active_embedding_profile: default_memory_active_embedding_profile(),
            graph_depth: 2,
            consolidation_enabled: true,
        }
    }
}

#[derive(Debug, Clone)]
struct RuntimeDiagnostics {
    persistence: String,
    embedding: String,
    embedding_probe: String,
    active_embedding_profile: String,
    embedding_dimension: Option<usize>,
    retrieval: String,
    last_error: Option<String>,
}

impl Default for RuntimeDiagnostics {
    fn default() -> Self {
        Self {
            persistence: "memory".to_string(),
            embedding: "unavailable".to_string(),
            embedding_probe: "not_checked".to_string(),
            active_embedding_profile: EMBEDDING_PROFILE_1536.to_string(),
            embedding_dimension: None,
            retrieval: "keyword_overlap".to_string(),
            last_error: None,
        }
    }
}

/// Built-in Memory Runtime.
pub struct AliaMemory {
    memories: Arc<RwLock<UserMemories>>,
    memory_write: Mutex<()>,
    privacy_states: RwLock<HashMap<Uuid, MemoryWriteContext>>,
    memory_sources: RwLock<HashMap<(Uuid, String), Vec<(String, i64)>>>,
    db: Arc<RwLock<Option<DatabaseConnection>>>,
    embedding: Arc<RwLock<Option<Arc<dyn EmbeddingAdapter>>>>,
    config: Arc<RwLock<MemoryRuntimeConfig>>,
    diagnostics: Arc<RwLock<RuntimeDiagnostics>>,
}

impl AliaMemory {
    pub fn new() -> Self {
        Self {
            memories: Arc::new(RwLock::new(HashMap::new())),
            memory_write: Mutex::new(()),
            privacy_states: RwLock::new(HashMap::new()),
            memory_sources: RwLock::new(HashMap::new()),
            db: Arc::new(RwLock::new(None)),
            embedding: Arc::new(RwLock::new(None)),
            config: Arc::new(RwLock::new(MemoryRuntimeConfig::default())),
            diagnostics: Arc::new(RwLock::new(RuntimeDiagnostics::default())),
        }
    }

    /// Attach production runtime dependencies without going through the full
    /// server builder. Integration tests use this to exercise PostgreSQL,
    /// pgvector, embeddings, retrieval, and graph projection end to end.
    pub async fn attach_runtime_for_test(
        &self,
        db: DatabaseConnection,
        embedding: Option<Arc<dyn EmbeddingAdapter>>,
        config: MemorySection,
    ) -> anyhow::Result<()> {
        *self.config.write().await = Self::from_config(&config);
        *self.embedding.write().await = embedding;
        Self::ensure_profile(&db).await?;
        *self.db.write().await = Some(db);
        {
            let mut diagnostics = self.diagnostics.write().await;
            diagnostics.persistence = "postgres".to_string();
        }
        self.refresh_embedding_health().await;
        Ok(())
    }

    fn from_config(config: &MemorySection) -> MemoryRuntimeConfig {
        MemoryRuntimeConfig {
            active_embedding_profile: config.active_embedding_profile.clone(),
            graph_depth: config.graph_depth.max(1),
            consolidation_enabled: config.consolidation_enabled,
        }
    }

    fn score_relevance(query: &str, memory: &MemoryNode) -> f32 {
        let query_words: Vec<String> = query
            .split_whitespace()
            .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
            .filter(|word| !word.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        if query_words.is_empty() {
            return 0.0;
        }

        let mut haystack = memory.content.to_ascii_lowercase();
        haystack.push(' ');
        haystack.push_str(&memory.topic_tags.join(" ").to_ascii_lowercase());
        haystack.push(' ');
        haystack.push_str(&memory.affect_tags.join(" ").to_ascii_lowercase());

        let matches = query_words
            .iter()
            .filter(|word| haystack.contains(word.as_str()))
            .count();
        let semantic = matches as f32 / query_words.len() as f32;
        (0.45 * semantic
            + 0.20 * memory.importance
            + 0.15 * memory.salience
            + 0.10 * memory.confidence)
            .clamp(0.0, 1.0)
    }

    async fn db(&self) -> Option<DatabaseConnection> {
        self.db.read().await.clone()
    }

    async fn validate_memory_context(&self, context: &MemoryWriteContext) -> anyhow::Result<()> {
        let states = self.privacy_states.read().await;
        let current = states
            .get(&context.user_id)
            .ok_or_else(|| anyhow::anyhow!("memory snapshot missing"))?;
        anyhow::ensure!(
            current.privacy_epoch == context.privacy_epoch,
            "memory privacy epoch changed"
        );
        anyhow::ensure!(
            context.source_received_at > current.source_cutoff_at,
            "memory source predates privacy boundary"
        );
        anyhow::ensure!(
            context.source_event_ids.is_empty(),
            "in-memory memory cannot verify persisted conversation sources"
        );
        let memories = self.memories.read().await;
        for (id, revision) in &context.source_memories {
            anyhow::ensure!(
                memories
                    .get(&context.user_id)
                    .is_some_and(|items| items.iter().any(|m| m.id.as_deref() == Some(id)
                        && m.revision == *revision
                        && memory_live(m))),
                "memory source changed, expired, deleted, or belongs to another user"
            );
        }
        Ok(())
    }

    /// Caller owns memory_write. Retain only erased IDs and revisions, never
    /// plaintext, and use the independent provenance map to find descendants.
    async fn erase_in_memory(&self, user_id: Uuid, ids: &[String]) -> anyhow::Result<u64> {
        let mut states = self.privacy_states.write().await;
        let current = states
            .entry(user_id)
            .or_insert_with(|| privacy::initial_context(user_id));
        let next_epoch = current
            .privacy_epoch
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("memory privacy epoch exhausted"))?;
        let mut sources = self.memory_sources.write().await;
        let mut erased = ids.iter().cloned().collect::<HashSet<_>>();
        loop {
            let previous = erased.len();
            for ((owner, id), dependencies) in sources.iter() {
                if *owner == user_id
                    && dependencies
                        .iter()
                        .any(|(source, _)| erased.contains(source))
                {
                    erased.insert(id.clone());
                }
            }
            if previous == erased.len() {
                break;
            }
        }
        let mut memories = self.memories.write().await;
        let items = memories.entry(user_id).or_default();
        for item in items.iter().filter(|m| {
            m.id.as_ref().is_some_and(|id| erased.contains(id)) && m.status != "deleted"
        }) {
            anyhow::ensure!(item.revision < i64::MAX, "memory revision exhausted");
        }
        let mut count = 0;
        for item in items.iter_mut().filter(|m| {
            m.id.as_ref().is_some_and(|id| erased.contains(id)) && m.status != "deleted"
        }) {
            item.revision += 1;
            item.status = "deleted".into();
            item.content.clear();
            item.summary = None;
            item.metadata = serde_json::json!({});
            item.evidence = serde_json::json!({});
            item.topic_tags.clear();
            item.affect_tags.clear();
            item.source_event_ids.clear();
            item.superseded_by = None;
            item.expires_at = Some(chrono::Utc::now());
            count += 1;
        }
        sources.retain(|(owner, id), _| *owner != user_id || !erased.contains(id));
        current.privacy_epoch = next_epoch;
        current.source_cutoff_at = chrono::Utc::now();
        Ok(count)
    }

    async fn erase_matching(
        &self,
        user_id: Uuid,
        topic: Option<&str>,
        kind: Option<&str>,
        hidden: Option<bool>,
    ) -> anyhow::Result<u64> {
        if let Some(db) = self.db().await {
            let tx = db.begin().await?;
            privacy::lock_user(&tx, user_id).await?;
            let rows=tx.query_all(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "SELECT id::text FROM memory_items WHERE user_id=$1 AND tenant_id=$2 AND status<>'deleted' AND ($3::text IS NULL OR strpos(lower(content),lower($3))>0 OR EXISTS(SELECT 1 FROM unnest(topic_tags) tag WHERE lower(tag)=lower($3))) AND ($4::text IS NULL OR kind=lower($4)) AND ($5::boolean IS NULL OR visibility='private_runtime' OR (sensitive AND visibility<>'user_visible'))",
                vec![user_id.into(),DEFAULT_TENANT_ID.into(),topic.map(str::to_string).into(),kind.map(str::to_string).into(),hidden.into()])).await?;
            let ids = rows
                .into_iter()
                .map(|row| row.try_get::<String>("", "id"))
                .collect::<Result<Vec<_>, _>>()?;
            let count = privacy::erase(&tx, user_id, &ids).await?;
            tx.commit().await?;
            return Ok(count);
        }
        let _guard = self.memory_write.lock().await;
        let ids = self
            .memories
            .read()
            .await
            .get(&user_id)
            .into_iter()
            .flatten()
            .filter(|m| {
                m.status != "deleted"
                    && topic.is_none_or(|topic| {
                        m.content.to_lowercase().contains(&topic.to_lowercase())
                            || m.topic_tags
                                .iter()
                                .any(|tag| tag.to_lowercase() == topic.to_lowercase())
                    })
                    && kind.is_none_or(|kind| m.kind == kind.to_ascii_lowercase())
                    && hidden.is_none_or(|_| {
                        m.visibility == "private_runtime"
                            || (m.sensitive && m.visibility != "user_visible")
                    })
            })
            .filter_map(|m| m.id.clone())
            .collect::<Vec<_>>();
        self.erase_in_memory(user_id, &ids).await
    }

    async fn commit_consolidation(
        &self,
        context: &MemoryWriteContext,
        originals: &[MemoryNode],
        mut patches: BTreeMap<String, MemoryPatch>,
        created: Vec<(MemoryNode, Vec<(String, i64)>)>,
    ) -> anyhow::Result<()> {
        let mut dependencies = HashMap::<String, Vec<(String, i64)>>::new();
        for (id, patch) in &mut patches {
            let original = originals
                .iter()
                .find(|node| node.id.as_ref() == Some(id))
                .ok_or_else(|| anyhow::anyhow!("consolidation target not in snapshot"))?;
            patch.expected_revision = Some(original.revision);
            if let Some(source) = patch
                .metadata
                .as_ref()
                .and_then(|m| m.get("related_memory_id"))
                .and_then(Value::as_str)
                && source != id
            {
                let original = originals
                    .iter()
                    .find(|node| node.id.as_deref() == Some(source))
                    .ok_or_else(|| anyhow::anyhow!("consolidation source not in snapshot"))?;
                dependencies
                    .entry(id.clone())
                    .or_default()
                    .push((source.to_string(), original.revision));
            }
        }
        if let Some(db) = self.db().await {
            let tx = db.begin().await?;
            // One immutable input snapshot and one commit: our own first update
            // cannot invalidate a later operation in this deterministic batch.
            privacy::validate(&tx, context).await?;
            let mut projected = Vec::new();
            for (node, sources) in created {
                let id = self.store_in_db(&tx, context.user_id, node, false).await?;
                dependencies.entry(id.clone()).or_default().extend(sources);
                projected.push(self.get_from_db(&tx, context.user_id, &id).await?.unwrap());
            }
            for (id, patch) in patches {
                projected.push(
                    self.update_in_db(&tx, context.user_id, &id, patch, false)
                        .await?,
                );
            }
            for (id, sources) in dependencies {
                for (source, revision) in sources {
                    tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                        "INSERT INTO memory_sources(memory_id,source_id,source_revision) VALUES($1::uuid,$2::uuid,$3) ON CONFLICT(memory_id,source_id) DO UPDATE SET source_revision=EXCLUDED.source_revision",
                        vec![id.clone().into(),source.into(),revision.into()])).await?;
                }
            }
            tx.commit().await?;
            for node in projected {
                self.write_embedding_if_available(&db, context, &node).await;
            }
            return Ok(());
        }
        let _guard = self.memory_write.lock().await;
        self.validate_memory_context(context).await?;
        let mut memories = self.memories.write().await;
        let mut working = memories.get(&context.user_id).cloned().unwrap_or_default();
        for (mut node, sources) in created {
            let id = node
                .id
                .clone()
                .ok_or_else(|| anyhow::anyhow!("planned memory lacks id"))?;
            anyhow::ensure!(
                !memories
                    .values()
                    .flatten()
                    .any(|m| m.id.as_ref() == Some(&id)),
                "planned memory id already exists"
            );
            node.normalize_runtime_defaults();
            prepare_memory_for_runtime(&mut node, "in_memory");
            node.revision = 1;
            dependencies.entry(id).or_default().extend(sources);
            working.push(node);
        }
        for (id, patch) in patches {
            let node = working
                .iter_mut()
                .find(|node| node.id.as_ref() == Some(&id))
                .ok_or_else(|| anyhow::anyhow!("planned memory missing"))?;
            apply_planned_patch(node, patch)?;
        }
        // Every fallible check finishes before either canonical map is replaced.
        let mut sources = self.memory_sources.write().await;
        for (id, links) in dependencies {
            sources
                .entry((context.user_id, id))
                .or_default()
                .extend(links);
        }
        memories.insert(context.user_id, working);
        Ok(())
    }

    async fn embedding(&self) -> Option<Arc<dyn EmbeddingAdapter>> {
        self.embedding.read().await.clone()
    }

    async fn active_profile(&self) -> String {
        self.config.read().await.active_embedding_profile.clone()
    }

    async fn should_use_1536_embedding(&self) -> bool {
        self.active_profile().await == EMBEDDING_PROFILE_1536
            && self
                .embedding()
                .await
                .is_some_and(|embedding| embedding.dimension() == 1536)
    }

    async fn refresh_embedding_health(&self) {
        let profile = self.active_profile().await;
        let embedding = self.embedding().await;
        let mut diagnostics = self.diagnostics.write().await;
        diagnostics.active_embedding_profile = profile.clone();
        diagnostics.embedding_dimension = embedding.as_ref().map(|embedding| embedding.dimension());
        match embedding {
            None => {
                diagnostics.embedding = "unavailable".to_string();
                diagnostics.retrieval = "keyword_overlap".to_string();
                diagnostics.embedding_probe = "no_embedding_adapter".to_string();
            }
            Some(embedding)
                if profile == EMBEDDING_PROFILE_1536 && embedding.dimension() == 1536 =>
            {
                diagnostics.embedding = "available".to_string();
                diagnostics.embedding_probe = "dimension_matches_active_profile".to_string();
                if diagnostics.persistence == "postgres" {
                    diagnostics.retrieval = "hybrid_vector_graph_policy".to_string();
                }
            }
            Some(embedding) => {
                diagnostics.embedding = "unsupported_profile_or_dimension".to_string();
                diagnostics.retrieval = "keyword_overlap".to_string();
                diagnostics.embedding_probe = format!(
                    "active_profile={profile}, adapter_dimension={}",
                    embedding.dimension()
                );
            }
        }
    }

    async fn mark_degraded(&self, reason: impl Into<String>) {
        let mut diagnostics = self.diagnostics.write().await;
        diagnostics.last_error = Some(reason.into());
    }

    async fn ensure_profile(db: &DatabaseConnection) -> anyhow::Result<()> {
        db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO embedding_profiles
                (id, provider, model, dimensions, distance, vector_type)
            VALUES
                ($1, 'openai', 'text-embedding-3-small', 1536, 'cosine', 'vector(1536)')
            ON CONFLICT (id) DO NOTHING
            "#,
            vec![EMBEDDING_PROFILE_1536.into()],
        ))
        .await?;
        for (id, provider, model, dimensions, distance, vector_type) in [
            (
                "openai.text-embedding-3-large.3072.cosine.v1",
                "openai",
                "text-embedding-3-large",
                3072,
                "cosine",
                "vector(3072)",
            ),
            (
                "generic.embedding.1024.cosine.v1",
                "generic",
                "embedding-1024",
                1024,
                "cosine",
                "vector(1024)",
            ),
        ] {
            db.execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                INSERT INTO embedding_profiles
                    (id, provider, model, dimensions, distance, vector_type)
                VALUES
                    ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (id) DO NOTHING
                "#,
                vec![
                    id.into(),
                    provider.into(),
                    model.into(),
                    dimensions.into(),
                    distance.into(),
                    vector_type.into(),
                ],
            ))
            .await?;
        }
        Ok(())
    }

    async fn update_in_db(
        &self,
        db: &(impl ConnectionTrait + Sync),
        user_id: Uuid,
        node_id: &str,
        patch: MemoryPatch,
        require_live_superseder: bool,
    ) -> anyhow::Result<MemoryNode> {
        let expected = patch
            .expected_revision
            .ok_or_else(|| anyhow::anyhow!("memory update requires expected revision"))?;
        if let Some(target) = &patch.superseded_by {
            anyhow::ensure!(
                self.get_from_db(db, user_id, target).await?.is_some_and(
                    |m| m.status != "deleted" && (!require_live_superseder || memory_live(&m))
                ),
                "superseding memory must be live and owned by the same user"
            );
        }
        let result = db.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "UPDATE memory_items SET content=COALESCE($4,content),summary=CASE WHEN $4::text IS NULL THEN summary ELSE NULL END,metadata=COALESCE($5::jsonb,metadata),importance=COALESCE($6,importance),confidence=COALESCE($7,confidence),salience=COALESCE($8,salience),visibility=COALESCE($9,visibility),status=COALESCE($10,status),superseded_by=COALESCE($11::uuid,superseded_by),revision=revision+1,updated_at=clock_timestamp() WHERE id=$1::uuid AND user_id=$2 AND tenant_id=$3 AND revision=$12 AND status<>'deleted'",
                vec![node_id.to_string().into(),user_id.into(),DEFAULT_TENANT_ID.into(),patch.content.into(),patch.metadata.into(),patch.importance.into(),patch.confidence.into(),patch.salience.into(),patch.visibility.into(),patch.status.into(),patch.superseded_by.into(),expected.into()])).await?;
        anyhow::ensure!(
            result.rows_affected() == 1,
            "memory was changed, deleted, or belongs to another user"
        );
        db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM memory_embeddings_1536 WHERE memory_id=$1::uuid",
            [node_id.to_string().into()],
        ))
        .await?;
        db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM memory_edges WHERE evidence_memory_id=$1::uuid AND user_id=$2",
            vec![node_id.to_string().into(), user_id.into()],
        ))
        .await?;
        let stored = self
            .get_from_db(db, user_id, node_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("updated memory missing"))?;
        if memory_live(&stored) {
            self.project_graph_for_memory_inner(db, user_id, &stored)
                .await?;
        }
        Ok(stored)
    }

    async fn store_in_memory(&self, user_id: Uuid, mut node: MemoryNode) -> anyhow::Result<String> {
        node.normalize_runtime_defaults();
        prepare_memory_for_runtime(&mut node, "in_memory");
        let id = match node.id.as_deref() {
            Some(id) => Uuid::parse_str(id)?.to_string(),
            None => Uuid::new_v4().to_string(),
        };
        let mut memories = self.memories.write().await;
        anyhow::ensure!(
            !memories
                .values()
                .flatten()
                .any(|item| item.id.as_deref() == Some(&id)),
            "memory id already exists"
        );
        if node.id.is_none()
            && self.config.read().await.consolidation_enabled
            && matches!(node.kind.as_str(), "preference" | "boundary" | "project")
            && let Some(existing) = memories.get_mut(&user_id).and_then(|items| {
                items.iter_mut().find(|m| {
                    memory_live(m)
                        && m.kind == node.kind
                        && memory_merge_key(&m.content) == memory_merge_key(&node.content)
                })
            })
        {
            existing.revision = existing
                .revision
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("memory revision exhausted"))?;
            existing.confidence = (existing.confidence.max(node.confidence) + 0.04).min(1.0);
            existing.salience = (existing.salience.max(node.salience) + 0.03).min(1.0);
            existing.importance = existing.importance.max(node.importance);
            return Ok(existing.id.clone().unwrap());
        }
        node.id = Some(id.clone());
        node.revision = 1;
        memories.entry(user_id).or_default().push(node);
        Ok(id)
    }

    async fn store_in_db(
        &self,
        db: &(impl ConnectionTrait + Sync),
        user_id: Uuid,
        mut node: MemoryNode,
        auto_consolidate: bool,
    ) -> anyhow::Result<String> {
        node.normalize_runtime_defaults();
        prepare_memory_for_runtime(&mut node, "postgres");
        let allow_merge = node.id.is_none();
        let id = match node.id.as_deref() {
            Some(id) => Uuid::parse_str(id)?,
            None => Uuid::new_v4(),
        };
        node.id = Some(id.to_string());

        if auto_consolidate
            && allow_merge
            && self.config.read().await.consolidation_enabled
            && let Some(existing) = self.find_merge_target_in_db(db, user_id, &node).await?
        {
            self.reinforce_existing_memory(db, user_id, existing, &node)
                .await?;
            let current = self
                .get_from_db(db, user_id, &existing.to_string())
                .await?
                .unwrap();
            self.project_graph_for_memory_inner(db, user_id, &current)
                .await?;
            return Ok(existing.to_string());
        }

        let topic_tags = serde_json::json!(node.topic_tags);
        let affect_tags = serde_json::json!(node.affect_tags);
        let source_event_ids = serde_json::json!(
            node.source_event_ids
                .iter()
                .map(Uuid::to_string)
                .collect::<Vec<_>>()
        );
        let superseded_by = node.superseded_by.as_deref();
        let conversation_id = node
            .metadata
            .get("conversation_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .map(|value| value.to_string());

        db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO memory_items (
                id, tenant_id, user_id, conversation_id, kind, scope, memory_layer,
                node_type, content, summary, confidence, salience, importance,
                affect_tags, topic_tags, source_event_ids, evidence, metadata,
                status, superseded_by, visibility, sensitive, expires_at, updated_at
            )
            VALUES (
                $1::uuid, $2::uuid, $3::uuid, $4::uuid, $5, $6, $7,
                $8, $9, $10, $11, $12, $13,
                ARRAY(SELECT jsonb_array_elements_text($14::jsonb)),
                ARRAY(SELECT jsonb_array_elements_text($15::jsonb)),
                ARRAY(SELECT jsonb_array_elements_text($16::jsonb)::uuid),
                $17::jsonb, $18::jsonb, $19, $20::uuid, $21, $22, $23, NOW()
            )
            "#,
            vec![
                id.to_string().into(),
                DEFAULT_TENANT_ID.to_string().into(),
                user_id.to_string().into(),
                conversation_id.into(),
                node.kind.clone().into(),
                node.scope.clone().into(),
                node.memory_layer.clone().into(),
                node.node_type.clone().into(),
                node.content.clone().into(),
                node.summary.clone().into(),
                node.confidence.into(),
                node.salience.into(),
                node.importance.into(),
                affect_tags.into(),
                topic_tags.into(),
                source_event_ids.into(),
                node.evidence.clone().into(),
                node.metadata.clone().into(),
                node.status.clone().into(),
                superseded_by.map(str::to_string).into(),
                node.visibility.clone().into(),
                node.sensitive.into(),
                node.expires_at.into(),
            ],
        ))
        .await?;

        self.project_graph_for_memory_inner(db, user_id, &node)
            .await?;
        if auto_consolidate && node.kind == "preference" {
            self.project_superseded_preferences_inner(db, user_id, id, &node)
                .await?;
        }
        Ok(id.to_string())
    }

    async fn find_merge_target_in_db(
        &self,
        db: &(impl ConnectionTrait + Sync),
        user_id: Uuid,
        incoming: &MemoryNode,
    ) -> anyhow::Result<Option<Uuid>> {
        if !matches!(
            incoming.kind.as_str(),
            "preference" | "boundary" | "project"
        ) {
            return Ok(None);
        }
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                SELECT id::text, content
                FROM memory_items
                WHERE tenant_id = $1::uuid
                  AND user_id = $2::uuid
                  AND kind = $3
                  AND status = 'active'
                  AND (expires_at IS NULL OR expires_at>clock_timestamp())
                ORDER BY updated_at DESC
                LIMIT 100
                "#,
                vec![
                    DEFAULT_TENANT_ID.to_string().into(),
                    user_id.to_string().into(),
                    incoming.kind.clone().into(),
                ],
            ))
            .await?;
        let incoming_key = memory_merge_key(&incoming.content);
        for row in rows {
            let content: String = row.try_get("", "content")?;
            if memory_merge_key(&content) == incoming_key {
                let id: String = row.try_get("", "id")?;
                return Ok(Some(Uuid::parse_str(&id)?));
            }
        }
        Ok(None)
    }

    async fn reinforce_existing_memory(
        &self,
        db: &(impl ConnectionTrait + Sync),
        user_id: Uuid,
        existing: Uuid,
        incoming: &MemoryNode,
    ) -> anyhow::Result<()> {
        db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            UPDATE memory_items
            SET confidence = LEAST(1.0, GREATEST(confidence, $2) + 0.04),
                salience = LEAST(1.0, GREATEST(salience, $3) + 0.03),
                importance = LEAST(1.0, GREATEST(importance, $4)),
                evidence = jsonb_set(
                    COALESCE(evidence, '{}'::jsonb),
                    '{evidence_count}',
                    to_jsonb(COALESCE((evidence->>'evidence_count')::integer, 1) + 1),
                    true
                ),
                metadata = COALESCE(metadata, '{}'::jsonb) || $5::jsonb,
                revision = revision + 1, updated_at = NOW()
            WHERE id = $1::uuid AND user_id = $6::uuid AND tenant_id = $7::uuid AND status = 'active'
            "#,
            vec![
                existing.to_string().into(),
                incoming.confidence.into(),
                incoming.salience.into(),
                incoming.importance.into(),
                merge_evidence_metadata(
                    incoming.metadata.clone(),
                    incoming.confidence,
                    incoming.importance,
                )
                .into(),
                user_id.into(),
                DEFAULT_TENANT_ID.into(),
            ],
        ))
        .await?;
        Ok(())
    }

    async fn project_superseded_preferences_inner(
        &self,
        db: &(impl ConnectionTrait + Sync),
        user_id: Uuid,
        new_id: Uuid,
        node: &MemoryNode,
    ) -> anyhow::Result<()> {
        let topic_tags = serde_json::json!(node.topic_tags);
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                SELECT id::text, content
                FROM memory_items
                WHERE tenant_id = $1::uuid
                  AND user_id = $2::uuid
                  AND kind = 'preference'
                  AND status = 'active'
                  AND id <> $3::uuid
                  AND topic_tags && ARRAY(SELECT jsonb_array_elements_text($4::jsonb))
                ORDER BY updated_at DESC
                LIMIT 10
                "#,
                vec![
                    DEFAULT_TENANT_ID.to_string().into(),
                    user_id.to_string().into(),
                    new_id.to_string().into(),
                    topic_tags.into(),
                ],
            ))
            .await?;

        for row in rows {
            let old_id = Uuid::parse_str(&row.try_get::<String>("", "id")?)?;
            let old_content: String = row.try_get("", "content")?;
            if !preference_supersedes(&node.content, &old_content) {
                continue;
            }

            db.execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                UPDATE memory_items
                SET status = 'superseded',
                    superseded_by = $2::uuid,
                    revision=revision+1, updated_at = NOW()
                WHERE id = $1::uuid AND user_id=$3 AND tenant_id=$4
                "#,
                vec![
                    old_id.to_string().into(),
                    new_id.to_string().into(),
                    user_id.into(),
                    DEFAULT_TENANT_ID.into(),
                ],
            ))
            .await?;

            let new_graph_node = upsert_graph_node(
                db,
                user_id,
                "preference",
                &format!("memory:{new_id}"),
                node.summary.as_deref().unwrap_or(&node.content),
                serde_json::json!({ "memory_id": new_id, "kind": node.kind }),
            )
            .await?;
            let old_graph_node = upsert_graph_node(
                db,
                user_id,
                "preference",
                &format!("memory:{old_id}"),
                &old_content,
                serde_json::json!({ "memory_id": old_id, "kind": "preference" }),
            )
            .await?;
            upsert_graph_edge(
                db,
                user_id,
                new_graph_node,
                old_graph_node,
                "SUPERSEDES",
                node.confidence,
                new_id,
            )
            .await?;
            upsert_graph_edge(
                db,
                user_id,
                new_graph_node,
                old_graph_node,
                "CONTRADICTS",
                node.confidence.min(0.85),
                new_id,
            )
            .await?;
        }
        Ok(())
    }

    async fn write_embedding_if_available(
        &self,
        db: &DatabaseConnection,
        context: &MemoryWriteContext,
        node: &MemoryNode,
    ) {
        if !memory_live(node) || !self.should_use_1536_embedding().await {
            return;
        }
        let Some(embedding) = self.embedding().await else {
            return;
        };
        let profile = self.active_profile().await;
        // No database or privacy lock is held while awaiting external inference.
        let result: anyhow::Result<()> = async {
            let vectors=embedding.embed(&[node.content.clone()]).await?;
            let vector=vectors.first().filter(|v|v.len()==1536).ok_or_else(||anyhow::anyhow!("embedding dimension mismatch"))?;
            anyhow::ensure!(vector.iter().all(|v|v.is_finite()),"embedding contains nonfinite values");
            let tx=db.begin().await?;
            // The stored item revision, not the old extractor's source snapshot,
            // identifies this projection. The epoch still invalidates deletion.
            let current=privacy::lock_user(&tx,context.user_id).await?;
            anyhow::ensure!(current.privacy_epoch==context.privacy_epoch,"late embedding rejected after privacy change");
            let id=node.id.as_ref().ok_or_else(||anyhow::anyhow!("memory id missing"))?;
            let updated=tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                "INSERT INTO memory_embeddings_1536(memory_id,profile_id,embedding,source_revision) SELECT id,$2,$3::vector,revision FROM memory_items WHERE id=$1::uuid AND user_id=$4 AND tenant_id=$5 AND revision=$6 AND status='active' AND (expires_at IS NULL OR expires_at>clock_timestamp()) ON CONFLICT(memory_id,profile_id) DO UPDATE SET embedding=EXCLUDED.embedding,source_revision=EXCLUDED.source_revision,created_at=clock_timestamp()",
                vec![id.clone().into(),profile.into(),vector_literal(vector).into(),context.user_id.into(),DEFAULT_TENANT_ID.into(),node.revision.into()])).await?;
            anyhow::ensure!(updated.rows_affected()==1,"late embedding rejected after memory revision change");
            tx.commit().await?;
            Ok(())
        }.await;
        let mut diagnostics = self.diagnostics.write().await;
        match result {
            Ok(()) => {
                diagnostics.embedding = "available".into();
                diagnostics.retrieval = "hybrid_vector_graph_policy".into();
            }
            Err(error) => {
                diagnostics.embedding = "write_failed".into();
                diagnostics.retrieval = "keyword_overlap".into();
                diagnostics.last_error = Some(error.to_string());
            }
        }
    }

    async fn project_graph_for_memory_inner(
        &self,
        db: &(impl ConnectionTrait + Sync),
        user_id: Uuid,
        node: &MemoryNode,
    ) -> anyhow::Result<()> {
        let Some(memory_id) = node.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) else {
            return Ok(());
        };
        let user_node = upsert_graph_node(
            db,
            user_id,
            "user",
            &format!("user:{user_id}"),
            "User",
            serde_json::json!({ "user_id": user_id }),
        )
        .await?;
        let graph_type = match node.kind.as_str() {
            "preference" => "preference",
            "boundary" => "boundary",
            "project" => "project",
            "affective_pattern" => "affect_pattern",
            _ => "memory",
        };
        let memory_node = upsert_graph_node(
            db,
            user_id,
            graph_type,
            &format!("memory:{memory_id}"),
            node.summary.as_deref().unwrap_or(&node.content),
            serde_json::json!({
                "memory_id": memory_id,
                "kind": node.kind,
                "visibility": node.visibility,
            }),
        )
        .await?;
        let relation = match node.kind.as_str() {
            "preference" if preference_is_dislike(&node.content) => "DISLIKES",
            "preference" => "PREFERS",
            "boundary" => "BOUNDARY_FOR",
            "project" => "WORKING_ON",
            "affective_pattern" => "TRIGGERED_BY",
            _ => "MENTIONED",
        };
        upsert_graph_edge(
            db,
            user_id,
            user_node,
            memory_node,
            relation,
            node.confidence,
            memory_id,
        )
        .await?;

        for topic in normalized_topic_tags(node) {
            let topic_node = upsert_graph_node(
                db,
                user_id,
                "topic",
                &format!("topic:{topic}"),
                &topic,
                serde_json::json!({ "topic": topic }),
            )
            .await?;
            upsert_graph_edge(
                db,
                user_id,
                memory_node,
                topic_node,
                "RELATED_TO",
                node.confidence,
                memory_id,
            )
            .await?;
            upsert_graph_edge(
                db,
                user_id,
                user_node,
                topic_node,
                "MENTIONED",
                node.confidence.min(0.8),
                memory_id,
            )
            .await?;
        }

        for entity in graph_entities_for_memory(node) {
            let entity_key = normalize_graph_key(&entity);
            if entity_key.is_empty() {
                continue;
            }
            let entity_node = upsert_graph_node(
                db,
                user_id,
                "entity",
                &format!("entity:{entity_key}"),
                &entity,
                serde_json::json!({ "entity": entity }),
            )
            .await?;
            upsert_graph_edge(
                db,
                user_id,
                memory_node,
                entity_node,
                "MENTIONED",
                node.confidence,
                memory_id,
            )
            .await?;
        }

        for device in graph_devices_for_memory(node) {
            let device_key = normalize_graph_key(&device);
            if device_key.is_empty() {
                continue;
            }
            let device_node = upsert_graph_node(
                db,
                user_id,
                "device",
                &format!("device:{device_key}"),
                &device,
                serde_json::json!({ "device": device }),
            )
            .await?;
            upsert_graph_edge(
                db,
                user_id,
                user_node,
                device_node,
                "USES_DEVICE",
                node.confidence,
                memory_id,
            )
            .await?;
        }
        Ok(())
    }

    async fn list_from_db(
        &self,
        db: &DatabaseConnection,
        user_id: Uuid,
        opts: MemoryListOpts,
    ) -> anyhow::Result<Vec<MemoryNode>> {
        let limit = opts.max_results.unwrap_or(100).clamp(1, 500) as i64;
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                SELECT id::text, revision, kind, scope, memory_layer, node_type, content, summary,
                       confidence, salience, importance, visibility, sensitive, status,
                       to_jsonb(topic_tags) AS topic_tags,
                       to_jsonb(affect_tags) AS affect_tags,
                       to_jsonb(source_event_ids) AS source_event_ids,
                       evidence, metadata, superseded_by::text, expires_at
                FROM memory_items
                WHERE tenant_id = $1::uuid
                  AND user_id = $2::uuid
                  AND status = 'active'
                  AND ($3::text[] IS NULL OR memory_layer = ANY($3::text[]))
                  AND (expires_at IS NULL OR expires_at > NOW())
                ORDER BY importance DESC, salience DESC, updated_at DESC
                LIMIT $4
                "#,
                vec![
                    DEFAULT_TENANT_ID.to_string().into(),
                    user_id.to_string().into(),
                    opts.layers.into(),
                    limit.into(),
                ],
            ))
            .await?;
        rows.into_iter().map(row_to_memory_node).collect()
    }

    async fn get_from_db(
        &self,
        db: &(impl ConnectionTrait + Sync),
        user_id: Uuid,
        node_id: &str,
    ) -> anyhow::Result<Option<MemoryNode>> {
        let row = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                SELECT id::text, revision, kind, scope, memory_layer, node_type, content, summary,
                       confidence, salience, importance, visibility, sensitive, status,
                       to_jsonb(topic_tags) AS topic_tags,
                       to_jsonb(affect_tags) AS affect_tags,
                       to_jsonb(source_event_ids) AS source_event_ids,
                       evidence, metadata, superseded_by::text, expires_at
                FROM memory_items
                WHERE tenant_id = $1::uuid
                  AND user_id = $2::uuid
                  AND id = $3::uuid
                LIMIT 1
                "#,
                vec![
                    DEFAULT_TENANT_ID.to_string().into(),
                    user_id.to_string().into(),
                    node_id.to_string().into(),
                ],
            ))
            .await?;
        row.map(row_to_memory_node).transpose()
    }

    async fn retrieve_from_db(
        &self,
        db: &DatabaseConnection,
        user_id: Uuid,
        query: &str,
        opts: RetrievalOpts,
    ) -> anyhow::Result<Vec<MemoryNode>> {
        if self.should_use_1536_embedding().await
            && let Some(embedding) = self.embedding().await
            && let Ok(vectors) = embedding.embed(&[query.to_string()]).await
            && let Some(vector) = vectors.first()
            && vector.len() == 1536
        {
            let vector_literal = vector_literal(vector);
            let rows = db
                .query_all(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    r#"
                    WITH ranked AS (
                    SELECT m.id::text, m.revision, m.kind, m.scope, m.memory_layer, m.node_type,
                           m.content, m.summary, m.confidence, m.salience, m.importance,
                           m.visibility, m.sensitive, m.status,
                           to_jsonb(m.topic_tags) AS topic_tags,
                           to_jsonb(m.affect_tags) AS affect_tags,
                           to_jsonb(m.source_event_ids) AS source_event_ids,
                           m.evidence, m.metadata, m.superseded_by::text, m.expires_at,
                           1 - (e.embedding <=> $3::vector) AS semantic_similarity,
                           GREATEST(0.0, 1.0 - EXTRACT(EPOCH FROM (NOW() - m.updated_at)) / 2592000.0) AS recency_score,
                           LEAST(1.0, COALESCE(edge_stats.edge_count, 0)::real / 3.0) AS graph_relevance,
                           CASE WHEN cardinality(m.affect_tags) > 0 THEN 0.5 ELSE 0.0 END AS affect_relevance
                    FROM memory_items m
                    JOIN memory_embeddings_1536 e ON e.memory_id = m.id AND e.source_revision = m.revision
                    LEFT JOIN LATERAL (
                        SELECT COUNT(*) AS edge_count
                        FROM memory_live_edges edge
                        WHERE edge.evidence_memory_id = m.id
                          AND edge.tenant_id = m.tenant_id
                          AND edge.user_id = m.user_id
                    ) edge_stats ON TRUE
                    WHERE m.tenant_id = $1::uuid
                      AND m.user_id = $2::uuid
                      AND e.profile_id = $4
                      AND m.status = 'active'
                      AND ($5::text[] IS NULL OR m.memory_layer = ANY($5::text[]))
                      AND (m.expires_at IS NULL OR m.expires_at > NOW())
                    )
                    SELECT *,
                           (
                               0.40 * semantic_similarity
                             + 0.15 * recency_score
                             + 0.15 * salience
                             + 0.10 * importance
                             + 0.10 * graph_relevance
                             + 0.05 * confidence
                             + 0.05 * affect_relevance
                           ) AS final_score
                    FROM ranked
                    ORDER BY final_score DESC, semantic_similarity DESC
                    LIMIT $6
                    "#,
                    vec![
                        DEFAULT_TENANT_ID.to_string().into(),
                        user_id.to_string().into(),
                        vector_literal.into(),
                        self.active_profile().await.into(),
                        opts.layers.clone().into(),
                        opts.max_results.unwrap_or(10).clamp(1, 50).into(),
                    ],
                ))
                .await?;
            let min_similarity = opts.min_similarity.unwrap_or(0.0).clamp(0.0, 1.0);
            let mut nodes = Vec::new();
            for row in rows {
                let similarity: f32 = row.try_get("", "semantic_similarity").unwrap_or(0.0);
                if similarity >= min_similarity {
                    let final_score: f32 = row.try_get("", "final_score").unwrap_or(similarity);
                    let recency_score: f32 = row.try_get("", "recency_score").unwrap_or(0.0);
                    let graph_relevance: f32 = row.try_get("", "graph_relevance").unwrap_or(0.0);
                    let affect_relevance: f32 = row.try_get("", "affect_relevance").unwrap_or(0.0);
                    let mut node = row_to_memory_node(row)?;
                    node.metadata = merge_retrieval_metadata(
                        node.metadata,
                        RetrievalScoreBreakdown {
                            retrieval_path: "vector_hnsw".to_string(),
                            semantic_similarity: similarity,
                            recency_score,
                            salience: node.salience,
                            importance: node.importance,
                            graph_relevance,
                            confidence: node.confidence,
                            affect_relevance,
                            final_score,
                            reason:
                                "Vector similarity matched the query and was reranked with memory policy signals."
                                    .to_string(),
                        },
                    );
                    nodes.push(node);
                }
            }
            if !nodes.is_empty() {
                return Ok(nodes);
            }
        }

        let candidates = self
            .list_from_db(
                db,
                user_id,
                MemoryListOpts {
                    max_results: Some(200),
                    layers: opts.layers.clone(),
                },
            )
            .await?;
        Ok(rank_keyword_candidates(query, candidates, opts))
    }

    async fn graph_from_db(
        &self,
        db: &DatabaseConnection,
        user_id: Uuid,
        opts: MemoryListOpts,
    ) -> anyhow::Result<GraphFragment> {
        let depth = self.config.read().await.graph_depth.clamp(1, 4) as i32;
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                WITH RECURSIVE graph_walk AS (
                    SELECT n.id, n.label, n.node_type, ARRAY[n.id] AS path, 0 AS depth
                    FROM memory_nodes n
                    WHERE n.tenant_id = $1::uuid
                      AND n.user_id = $2::uuid
                      AND n.node_type = 'user'
                    UNION ALL
                    SELECT next_node.id, next_node.label, next_node.node_type,
                           graph_walk.path || next_node.id,
                           graph_walk.depth + 1
                    FROM graph_walk
                    JOIN memory_live_edges e ON e.src_node_id = graph_walk.id
                    JOIN memory_nodes next_node ON next_node.id = e.dst_node_id
                    WHERE graph_walk.depth < $3
                      AND NOT next_node.id = ANY(graph_walk.path)
                      AND e.tenant_id = $1::uuid
                      AND e.user_id = $2::uuid
                      AND (e.valid_to IS NULL OR e.valid_to > NOW())
                )
                SELECT DISTINCT id::text, label, node_type
                FROM graph_walk
                WHERE depth > 0
                LIMIT $4
                "#,
                vec![
                    DEFAULT_TENANT_ID.to_string().into(),
                    user_id.to_string().into(),
                    depth.into(),
                    opts.max_results.unwrap_or(100).clamp(1, 500).into(),
                ],
            ))
            .await?;
        let nodes = rows
            .into_iter()
            .map(|row| {
                let id: String = row.try_get("", "id")?;
                let node_type: String = row.try_get("", "node_type")?;
                let content: String = row.try_get("", "label")?;
                Ok(MemoryNode::new("graph", node_type, content, 0.5)
                    .with_runtime_fields("semantic", "user", 0.6, 0.5, "internal")
                    .with_metadata(serde_json::json!({ "graph_node_id": id })))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let edge_rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                SELECT src_node_id::text, dst_node_id::text, relation, weight
                FROM memory_live_edges
                WHERE tenant_id = $1::uuid AND user_id = $2::uuid
                ORDER BY created_at DESC
                LIMIT $3
                "#,
                vec![
                    DEFAULT_TENANT_ID.to_string().into(),
                    user_id.to_string().into(),
                    opts.max_results.unwrap_or(100).clamp(1, 500).into(),
                ],
            ))
            .await?;
        let edges = edge_rows
            .into_iter()
            .map(|row| {
                Ok(MemoryEdge {
                    source_id: row.try_get("", "src_node_id")?,
                    target_id: row.try_get("", "dst_node_id")?,
                    relation_type: row.try_get("", "relation")?,
                    weight: row.try_get("", "weight")?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(GraphFragment { nodes, edges })
    }

    async fn graph_snippets_from_db(
        &self,
        db: &DatabaseConnection,
        user_id: Uuid,
    ) -> anyhow::Result<Vec<GraphSnippet>> {
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                r#"
                SELECT src.label AS source, e.relation, dst.label AS target, e.confidence
                FROM memory_live_edges e
                JOIN memory_nodes src ON src.id = e.src_node_id
                JOIN memory_nodes dst ON dst.id = e.dst_node_id
                WHERE e.tenant_id = $1::uuid
                  AND e.user_id = $2::uuid
                  AND (e.valid_to IS NULL OR e.valid_to > NOW())
                ORDER BY e.confidence DESC, e.created_at DESC
                LIMIT 24
                "#,
                vec![
                    DEFAULT_TENANT_ID.to_string().into(),
                    user_id.to_string().into(),
                ],
            ))
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(GraphSnippet {
                    source: row.try_get("", "source")?,
                    relation: row.try_get("", "relation")?,
                    target: row.try_get("", "target")?,
                    confidence: row.try_get("", "confidence")?,
                })
            })
            .collect()
    }
}

impl Default for AliaMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for AliaMemory {
    fn name(&self) -> &'static str {
        "alia_memory_runtime"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        *self.config.write().await = Self::from_config(&ctx.config.policies.memory);
        *self.embedding.write().await = ctx.embedding.clone();
        *self.db.write().await = Some(ctx.db.clone());
        Self::ensure_profile(&ctx.db).await?;
        self.diagnostics.write().await.persistence = "postgres".to_string();
        self.refresh_embedding_health().await;
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let diagnostics = self.diagnostics.read().await;
        if diagnostics.persistence == "postgres" && diagnostics.last_error.is_none() {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Degraded(diagnostics.last_error.clone().unwrap_or_else(|| {
                "memory runtime is using degraded storage or retrieval".to_string()
            }))
        }
    }

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let mut applied = Vec::new();
        let mut config = self.config.write().await;
        if let Some(value) = updates
            .get("core.policies.memory.active_embedding_profile")
            .and_then(Value::as_str)
        {
            config.active_embedding_profile = value.to_string();
            applied.push("core.policies.memory.active_embedding_profile".to_string());
        }
        if let Some(value) = updates
            .get("core.policies.memory.graph_depth")
            .and_then(Value::as_u64)
        {
            config.graph_depth = (value as u32).clamp(1, 4);
            applied.push("core.policies.memory.graph_depth".to_string());
        }
        if let Some(value) = updates
            .get("core.policies.memory.consolidation_enabled")
            .and_then(Value::as_bool)
        {
            config.consolidation_enabled = value;
            applied.push("core.policies.memory.consolidation_enabled".to_string());
        }
        drop(config);
        if !applied.is_empty() {
            self.refresh_embedding_health().await;
        }
        Ok(applied)
    }
}

#[async_trait]
impl MemoryBackend for AliaMemory {
    async fn write_snapshot(&self, user_id: Uuid) -> anyhow::Result<MemoryWriteContext> {
        if let Some(db) = self.db().await {
            let tx = db.begin().await?;
            let snapshot = privacy::lock_user(&tx, user_id).await?;
            tx.commit().await?;
            return Ok(snapshot);
        }
        let mut states = self.privacy_states.write().await;
        let mut context = states
            .entry(user_id)
            .or_insert_with(|| privacy::initial_context(user_id))
            .clone();
        context.source_received_at = chrono::Utc::now();
        Ok(context)
    }

    async fn store(&self, user_id: Uuid, node: MemoryNode) -> anyhow::Result<String> {
        self.store_guarded(&self.write_snapshot(user_id).await?, node)
            .await
    }

    async fn store_guarded(
        &self,
        context: &MemoryWriteContext,
        mut node: MemoryNode,
    ) -> anyhow::Result<String> {
        anyhow::ensure!(
            node.status != "deleted",
            "create cannot write a deletion tombstone"
        );
        if !node.metadata.is_object() {
            node.metadata = serde_json::json!({});
        }
        if !context.source_memories.is_empty() && node.id.is_none() {
            node.id = Some(Uuid::new_v4().to_string());
        }
        anyhow::ensure!(
            !context
                .source_memories
                .iter()
                .any(|(id, _)| node.id.as_ref() == Some(id)),
            "derived memory cannot overwrite its own source"
        );
        node.source_event_ids = context.source_event_ids.clone();
        node.metadata["source_received_at"] = serde_json::json!(context.source_received_at);
        node.metadata["source_memory_versions"] = serde_json::json!(context.source_memories);
        if let Some(db) = self.db().await {
            let tx = db.begin().await?;
            privacy::validate(&tx, context).await?;
            if let Some(target) = &node.superseded_by {
                anyhow::ensure!(
                    self.get_from_db(&tx, context.user_id, target)
                        .await?
                        .is_some_and(|m| memory_live(&m)),
                    "superseding memory must be live and owned by same user"
                );
            }
            let id = self.store_in_db(&tx, context.user_id, node, true).await?;
            for (source, revision) in &context.source_memories {
                anyhow::ensure!(
                    source != &id,
                    "derived memory cannot overwrite its own source"
                );
                tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,
                    "INSERT INTO memory_sources(memory_id,source_id,source_revision) VALUES($1::uuid,$2::uuid,$3) ON CONFLICT(memory_id,source_id) DO UPDATE SET source_revision=EXCLUDED.source_revision",
                    vec![id.clone().into(), source.clone().into(), (*revision).into()])).await?;
            }
            let stored = self
                .get_from_db(&tx, context.user_id, &id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("stored memory missing"))?;
            tx.commit().await?;
            self.write_embedding_if_available(&db, context, &stored)
                .await;
            return Ok(id);
        }
        let _guard = self.memory_write.lock().await;
        self.validate_memory_context(context).await?;
        if let Some(target) = &node.superseded_by {
            anyhow::ensure!(
                self.get(context.user_id, target)
                    .await?
                    .is_some_and(|m| memory_live(&m)),
                "superseding memory must be live and owned by same user"
            );
        }
        let id = self.store_in_memory(context.user_id, node).await?;
        self.memory_sources
            .write()
            .await
            .entry((context.user_id, id.clone()))
            .or_default()
            .extend(context.source_memories.clone());
        Ok(id)
    }

    async fn list(&self, user_id: Uuid, opts: MemoryListOpts) -> anyhow::Result<Vec<MemoryNode>> {
        if let Some(db) = self.db().await {
            return self.list_from_db(&db, user_id, opts).await;
        }
        let memories = self.memories.read().await;
        let mut results = memories
            .get(&user_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|memory| {
                memory_live(memory)
                    && opts
                        .layers
                        .as_ref()
                        .is_none_or(|layers| layers.contains(&memory.memory_layer))
            })
            .collect::<Vec<_>>();
        results.sort_by(|left, right| right.importance.total_cmp(&left.importance));
        results.truncate(opts.max_results.unwrap_or(100).clamp(1, 500) as usize);
        Ok(results)
    }

    async fn get(&self, user_id: Uuid, node_id: &str) -> anyhow::Result<Option<MemoryNode>> {
        let node = if let Some(db) = self.db().await {
            self.get_from_db(&db, user_id, node_id).await?
        } else {
            self.memories
                .read()
                .await
                .get(&user_id)
                .and_then(|items| items.iter().find(|m| m.id.as_deref() == Some(node_id)))
                .cloned()
        };
        Ok(node.filter(|node| node.status != "deleted"))
    }

    async fn retrieve(
        &self,
        user_id: Uuid,
        query: &str,
        opts: RetrievalOpts,
    ) -> anyhow::Result<Vec<MemoryNode>> {
        if let Some(db) = self.db().await {
            return self.retrieve_from_db(&db, user_id, query, opts).await;
        }
        let candidates = self
            .list(
                user_id,
                MemoryListOpts {
                    max_results: Some(500),
                    layers: opts.layers.clone(),
                },
            )
            .await?;
        Ok(rank_keyword_candidates(query, candidates, opts))
    }

    async fn update(
        &self,
        user_id: Uuid,
        node_id: &str,
        mut patch: MemoryPatch,
    ) -> anyhow::Result<()> {
        let context = self.write_snapshot(user_id).await?;
        if patch.expected_revision.is_none() {
            patch.expected_revision = Some(
                self.get(user_id, node_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("memory not found for owner"))?
                    .revision,
            );
        }
        self.update_guarded(&context, node_id, patch).await
    }

    async fn update_guarded(
        &self,
        context: &MemoryWriteContext,
        node_id: &str,
        patch: MemoryPatch,
    ) -> anyhow::Result<()> {
        let expected = patch
            .expected_revision
            .ok_or_else(|| anyhow::anyhow!("memory update requires expected revision"))?;
        if let Some(status) = &patch.status {
            anyhow::ensure!(
                matches!(
                    status.as_str(),
                    "active" | "expired" | "superseded" | "rejected"
                ),
                "invalid memory update status"
            );
        }
        if let Some(db) = self.db().await {
            let tx = db.begin().await?;
            privacy::validate(&tx, context).await?;
            let stored = self
                .update_in_db(&tx, context.user_id, node_id, patch, true)
                .await?;
            for (source, revision) in &context.source_memories {
                if source == node_id {
                    continue;
                }
                tx.execute(Statement::from_sql_and_values(DatabaseBackend::Postgres,"INSERT INTO memory_sources(memory_id,source_id,source_revision) VALUES($1::uuid,$2::uuid,$3) ON CONFLICT(memory_id,source_id) DO UPDATE SET source_revision=EXCLUDED.source_revision",vec![node_id.to_string().into(),source.clone().into(),(*revision).into()])).await?;
            }
            tx.commit().await?;
            self.write_embedding_if_available(&db, context, &stored)
                .await;
            return Ok(());
        }
        let _guard = self.memory_write.lock().await;
        self.validate_memory_context(context).await?;
        if let Some(target) = &patch.superseded_by {
            anyhow::ensure!(
                self.get(context.user_id, target)
                    .await?
                    .is_some_and(|m| memory_live(&m)),
                "superseding memory must be live and owned by same user"
            );
        }
        let mut memories = self.memories.write().await;
        let node = memories
            .get_mut(&context.user_id)
            .and_then(|items| items.iter_mut().find(|m| m.id.as_deref() == Some(node_id)))
            .ok_or_else(|| anyhow::anyhow!("memory not found for owner"))?;
        anyhow::ensure!(
            node.status != "deleted" && node.revision == expected,
            "memory revision changed or memory was deleted"
        );
        let next_revision = node
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("memory revision exhausted"))?;
        if let Some(content) = patch.content {
            node.content = content;
            node.summary = None;
        }
        if let Some(metadata) = patch.metadata {
            node.metadata = metadata;
        }
        if let Some(value) = patch.importance {
            node.importance = value;
        }
        if let Some(value) = patch.confidence {
            node.confidence = value;
        }
        if let Some(value) = patch.salience {
            node.salience = value;
        }
        if let Some(value) = patch.visibility {
            node.visibility = value;
        }
        if let Some(value) = patch.status {
            node.status = value;
        }
        if let Some(value) = patch.superseded_by {
            node.superseded_by = Some(value);
        }
        node.revision = next_revision;
        node.normalize_runtime_defaults();
        drop(memories);
        self.memory_sources
            .write()
            .await
            .entry((context.user_id, node_id.to_string()))
            .or_default()
            .extend(
                context
                    .source_memories
                    .iter()
                    .filter(|(id, _)| id != node_id)
                    .cloned(),
            );
        Ok(())
    }

    async fn delete(&self, user_id: Uuid, node_id: &str) -> anyhow::Result<()> {
        self.delete_guarded(&self.write_snapshot(user_id).await?, node_id)
            .await
    }

    async fn delete_guarded(
        &self,
        context: &MemoryWriteContext,
        node_id: &str,
    ) -> anyhow::Result<()> {
        if let Some(db) = self.db().await {
            let tx = db.begin().await?;
            privacy::validate(&tx, context).await?;
            let item = self
                .get_from_db(&tx, context.user_id, node_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("memory not found for owner"))?;
            if item.status != "deleted" {
                privacy::erase(&tx, context.user_id, &[node_id.to_string()]).await?;
            }
            tx.commit().await?;
            return Ok(());
        }
        let _guard = self.memory_write.lock().await;
        self.validate_memory_context(context).await?;
        anyhow::ensure!(
            self.memories
                .read()
                .await
                .get(&context.user_id)
                .is_some_and(|items| items.iter().any(|m| m.id.as_deref() == Some(node_id))),
            "memory not found for owner"
        );
        self.erase_in_memory(context.user_id, &[node_id.to_string()])
            .await?;
        Ok(())
    }

    async fn clear(&self, user_id: Uuid) -> anyhow::Result<u64> {
        self.erase_matching(user_id, None, None, None).await
    }

    async fn memory_capabilities(&self) -> MemoryCapabilities {
        self.refresh_embedding_health().await;
        let diagnostics = self.diagnostics.read().await.clone();
        let mut capabilities = MemoryCapabilities::minimal(self.name());
        capabilities.supports_search_filters = true;
        capabilities.supports_similarity_threshold = true;
        capabilities.supports_graph = true;
        capabilities.supports_relationship_edges = true;
        capabilities.max_content_chars = Some(16_000);
        capabilities.diagnostics.insert(
            "persistence".to_string(),
            serde_json::json!(diagnostics.persistence),
        );
        capabilities.diagnostics.insert(
            "embedding".to_string(),
            serde_json::json!(diagnostics.embedding),
        );
        capabilities.diagnostics.insert(
            "embedding_probe".to_string(),
            serde_json::json!(diagnostics.embedding_probe),
        );
        capabilities.diagnostics.insert(
            "active_embedding_profile".to_string(),
            serde_json::json!(diagnostics.active_embedding_profile),
        );
        capabilities.diagnostics.insert(
            "embedding_dimension".to_string(),
            serde_json::json!(diagnostics.embedding_dimension),
        );
        capabilities.diagnostics.insert(
            "retrieval".to_string(),
            serde_json::json!(diagnostics.retrieval),
        );
        if let Some(error) = diagnostics.last_error {
            capabilities
                .diagnostics
                .insert("last_error".to_string(), serde_json::json!(error));
        }
        capabilities
    }

    async fn graph_for_user(
        &self,
        user_id: Uuid,
        opts: MemoryListOpts,
    ) -> anyhow::Result<GraphFragment> {
        if let Some(db) = self.db().await {
            return self.graph_from_db(&db, user_id, opts).await;
        }
        let nodes = self.list(user_id, opts).await?;
        Ok(GraphFragment {
            nodes,
            edges: Vec::new(),
        })
    }

    async fn consolidate_user(&self, user_id: Uuid) -> anyhow::Result<MemoryConsolidationResult> {
        let mut context = self.write_snapshot(user_id).await?;
        let memories = self
            .list(
                user_id,
                MemoryListOpts {
                    max_results: Some(10_000),
                    layers: None,
                },
            )
            .await?;
        context.source_memories = memories
            .iter()
            .filter_map(|node| node.id.clone().map(|id| (id, node.revision)))
            .collect();
        let mut patches = BTreeMap::<String, MemoryPatch>::new();
        let mut created = Vec::<(MemoryNode, Vec<(String, i64)>)>::new();
        let mut result = MemoryConsolidationResult {
            scanned: memories.len(),
            ..MemoryConsolidationResult::default()
        };

        let mut by_merge_key: HashMap<(String, String), MemoryNode> = HashMap::new();
        for memory in memories.iter().filter(|memory| memory.status == "active") {
            let Some(id) = memory.id.clone() else {
                continue;
            };
            if matches!(
                memory.kind.as_str(),
                "preference" | "boundary" | "project" | "semantic"
            ) {
                let key = (memory.kind.clone(), memory_merge_key(&memory.content));
                if let Some(existing) = by_merge_key.get(&key) {
                    let Some(existing_id) = existing.id.clone() else {
                        continue;
                    };
                    let (keeper, duplicate) = if memory_quality(memory) > memory_quality(existing) {
                        (memory, existing)
                    } else {
                        (existing, memory)
                    };
                    let Some(keeper_id) = keeper.id.clone() else {
                        continue;
                    };
                    let Some(duplicate_id) = duplicate.id.clone() else {
                        continue;
                    };
                    queue_patch(
                        &mut patches,
                        &keeper_id,
                        MemoryPatch {
                            metadata: Some(consolidation_metadata(
                                keeper,
                                duplicate,
                                "deduplicated_and_reinforced",
                            )),
                            confidence: Some((keeper.confidence + 0.05).min(1.0)),
                            salience: Some(keeper.salience.max(duplicate.salience)),
                            importance: Some(keeper.importance.max(duplicate.importance)),
                            ..MemoryPatch::default()
                        },
                    );
                    queue_patch(
                        &mut patches,
                        &duplicate_id,
                        MemoryPatch {
                            status: Some("superseded".to_string()),
                            superseded_by: Some(keeper_id.clone()),
                            ..MemoryPatch::default()
                        },
                    );
                    if id == keeper_id {
                        by_merge_key.insert(key, keeper.clone());
                    } else if existing_id == keeper_id {
                        by_merge_key.insert(key, existing.clone());
                    }
                    result.deduplicated += 1;
                } else {
                    by_merge_key.insert(key, memory.clone());
                }
            }
        }

        for episode in memories.iter().filter(|memory| {
            memory.status == "active"
                && memory.kind == "episodic"
                && memory.confidence >= 0.65
                && memory.importance >= 0.45
                && content_signals_user_preference(&memory.content)
        }) {
            let Some(episode_id) = episode.id.clone() else {
                continue;
            };
            let mut promoted = MemoryNode::new(
                "preference",
                "preference",
                constructive_preference_content(&episode.content),
                episode.importance.max(0.55),
            )
            .with_runtime_fields(
                "preference",
                "user",
                episode.confidence,
                episode.salience.max(0.55),
                "internal",
            )
            .with_metadata(serde_json::json!({
                "source": "memory_consolidation",
                "promoted_from": episode_id,
                "reason": "Repeated or explicit preference-like episodic memory was promoted to a user preference.",
            }));
            promoted.topic_tags = episode.topic_tags.clone();
            promoted.affect_tags = episode.affect_tags.clone();
            promoted.evidence = serde_json::json!({
                "source": "memory_consolidation",
                "summary": "Episode promoted to preference because it contains stable user-preference language.",
                "evidence_memory_ids": [episode_id],
            });
            let promoted_id = Uuid::new_v4().to_string();
            promoted.id = Some(promoted_id.clone());
            promoted.source_event_ids = episode.source_event_ids.clone();
            created.push((promoted, vec![(episode_id.clone(), episode.revision)]));
            queue_patch(
                &mut patches,
                &episode_id,
                MemoryPatch {
                    status: Some("superseded".to_string()),
                    superseded_by: Some(promoted_id),
                    ..MemoryPatch::default()
                },
            );
            result.promoted += 1;
        }

        let active_preferences = memories
            .iter()
            .filter(|memory| memory.status == "active" && memory.kind == "preference")
            .collect::<Vec<_>>();
        for (index, left) in active_preferences.iter().enumerate() {
            let Some(left_id) = left.id.clone() else {
                continue;
            };
            for right in active_preferences.iter().skip(index + 1) {
                let Some(right_id) = right.id.clone() else {
                    continue;
                };
                if !memory_topic_overlap(left, right) {
                    continue;
                }
                if preference_is_dislike(&left.content) != preference_is_dislike(&right.content) {
                    result.contradictions += 1;
                    if memory_quality(left) >= memory_quality(right) {
                        queue_patch(
                            &mut patches,
                            &right_id,
                            MemoryPatch {
                                metadata: Some(consolidation_metadata(
                                    right,
                                    left,
                                    "contradicted_by_higher_quality_preference",
                                )),
                                status: Some("superseded".to_string()),
                                superseded_by: Some(left_id.clone()),
                                ..MemoryPatch::default()
                            },
                        );
                        result.superseded += 1;
                    } else {
                        queue_patch(
                            &mut patches,
                            &left_id,
                            MemoryPatch {
                                metadata: Some(consolidation_metadata(
                                    left,
                                    right,
                                    "contradicted_by_higher_quality_preference",
                                )),
                                status: Some("superseded".to_string()),
                                superseded_by: Some(right_id.clone()),
                                ..MemoryPatch::default()
                            },
                        );
                        result.superseded += 1;
                    }
                } else if preference_supersedes(&left.content, &right.content) {
                    queue_patch(
                        &mut patches,
                        &right_id,
                        MemoryPatch {
                            status: Some("superseded".to_string()),
                            superseded_by: Some(left_id.clone()),
                            ..MemoryPatch::default()
                        },
                    );
                    result.superseded += 1;
                } else if preference_supersedes(&right.content, &left.content) {
                    queue_patch(
                        &mut patches,
                        &left_id,
                        MemoryPatch {
                            status: Some("superseded".to_string()),
                            superseded_by: Some(right_id.clone()),
                            ..MemoryPatch::default()
                        },
                    );
                    result.superseded += 1;
                }
            }
        }

        for memory in memories.iter().filter(|memory| memory.status == "active") {
            let Some(id) = memory.id.clone() else {
                continue;
            };
            if matches!(memory.kind.as_str(), "preference" | "social" | "semantic")
                && (memory.confidence < 0.35 || memory.importance < 0.25)
            {
                queue_patch(
                    &mut patches,
                    &id,
                    MemoryPatch {
                        status: Some("expired".to_string()),
                        metadata: Some(consolidation_metadata(
                            memory,
                            memory,
                            "demoted_low_confidence_or_importance",
                        )),
                        ..MemoryPatch::default()
                    },
                );
                result.demoted += 1;
                continue;
            }

            if memory.kind == "affective_pattern" && is_negative_affective_memory(&memory.content) {
                queue_patch(
                    &mut patches,
                    &id,
                    MemoryPatch {
                        content: Some(constructive_affective_guidance(&memory.content)),
                        metadata: Some(consolidation_metadata(
                            memory,
                            memory,
                            "negative_affective_memory_transformed_to_constructive_guidance",
                        )),
                        ..MemoryPatch::default()
                    },
                );
                result.transformed_affective_guidance += 1;
            }
        }

        self.commit_consolidation(&context, &memories, patches, created)
            .await?;
        Ok(result)
    }

    async fn delete_by_topic(&self, user_id: Uuid, topic: &str) -> anyhow::Result<u64> {
        if topic.trim().is_empty() {
            return Ok(0);
        }
        self.erase_matching(user_id, Some(topic.trim()), None, None)
            .await
    }
    async fn delete_by_kind(&self, user_id: Uuid, kind: &str) -> anyhow::Result<u64> {
        if kind.trim().is_empty() {
            return Ok(0);
        }
        self.erase_matching(user_id, None, Some(kind.trim()), None)
            .await
    }
    async fn delete_hidden(&self, user_id: Uuid) -> anyhow::Result<u64> {
        self.erase_matching(user_id, None, None, Some(true)).await
    }

    async fn export_user(&self, user_id: Uuid) -> anyhow::Result<MemoryExport> {
        let memories = self
            .list(
                user_id,
                MemoryListOpts {
                    max_results: Some(10_000),
                    layers: None,
                },
            )
            .await?;
        let mut export = MemoryExport::default();
        for memory in memories {
            if let Some(reason) = hidden_memory_export_reason(&memory) {
                export.suppressed.push(SuppressedMemory {
                    memory_id: memory.id.unwrap_or_default(),
                    reason,
                });
            } else {
                export.memories.push(memory);
            }
        }
        Ok(export)
    }

    async fn retrieve_context_pack(
        &self,
        user_id: Uuid,
        query: &str,
        opts: RetrievalOpts,
    ) -> anyhow::Result<MemoryContextPack> {
        let nodes = self.retrieve(user_id, query, opts).await?;
        let graph_context = if let Some(db) = self.db().await {
            self.graph_snippets_from_db(&db, user_id).await?
        } else {
            Vec::new()
        };
        Ok(memory_context_pack_from_nodes(nodes, graph_context))
    }
}

fn memory_live(node: &MemoryNode) -> bool {
    node.status == "active"
        && node
            .expires_at
            .is_none_or(|expiry| expiry > chrono::Utc::now())
}

fn queue_patch(patches: &mut BTreeMap<String, MemoryPatch>, id: &str, incoming: MemoryPatch) {
    let patch = patches.entry(id.to_string()).or_default();
    macro_rules! merge {($($field:ident),*)=>{$(if incoming.$field.is_some(){patch.$field=incoming.$field;})*};}
    merge!(
        content,
        metadata,
        importance,
        confidence,
        salience,
        visibility,
        status,
        superseded_by
    );
}

fn apply_planned_patch(node: &mut MemoryNode, patch: MemoryPatch) -> anyhow::Result<()> {
    anyhow::ensure!(
        node.status != "deleted" && Some(node.revision) == patch.expected_revision,
        "planned memory revision changed"
    );
    let revision = node
        .revision
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("memory revision exhausted"))?;
    if let Some(value) = patch.content {
        node.content = value;
        node.summary = None;
    }
    if let Some(value) = patch.metadata {
        node.metadata = value;
    }
    if let Some(value) = patch.importance {
        node.importance = value;
    }
    if let Some(value) = patch.confidence {
        node.confidence = value;
    }
    if let Some(value) = patch.salience {
        node.salience = value;
    }
    if let Some(value) = patch.visibility {
        node.visibility = value;
    }
    if let Some(value) = patch.status {
        node.status = value;
    }
    if let Some(value) = patch.superseded_by {
        node.superseded_by = Some(value);
    }
    node.revision = revision;
    node.normalize_runtime_defaults();
    Ok(())
}

fn row_to_memory_node(row: sea_orm::QueryResult) -> anyhow::Result<MemoryNode> {
    let id: String = row.try_get("", "id")?;
    let revision: i64 = row.try_get("", "revision")?;
    let topic_tags: Value = row
        .try_get("", "topic_tags")
        .unwrap_or_else(|_| serde_json::json!([]));
    let affect_tags: Value = row
        .try_get("", "affect_tags")
        .unwrap_or_else(|_| serde_json::json!([]));
    let source_event_ids: Value = row
        .try_get("", "source_event_ids")
        .unwrap_or_else(|_| serde_json::json!([]));
    let superseded_by: Option<String> = row.try_get("", "superseded_by").ok().flatten();
    let mut node = MemoryNode {
        revision,
        id: Some(id),
        memory_layer: row.try_get("", "memory_layer")?,
        node_type: row.try_get("", "node_type")?,
        content: row.try_get("", "content")?,
        metadata: row.try_get("", "metadata")?,
        importance: row.try_get("", "importance")?,
        kind: row.try_get("", "kind")?,
        scope: row.try_get("", "scope")?,
        summary: row.try_get("", "summary").ok().flatten(),
        confidence: row.try_get("", "confidence")?,
        salience: row.try_get("", "salience")?,
        visibility: row.try_get("", "visibility")?,
        sensitive: row.try_get("", "sensitive")?,
        status: row.try_get("", "status")?,
        topic_tags: json_string_array(topic_tags),
        affect_tags: json_string_array(affect_tags),
        evidence: row.try_get("", "evidence")?,
        source_event_ids: json_uuid_array(source_event_ids),
        superseded_by,
        expires_at: row.try_get("", "expires_at").ok().flatten(),
    };
    node.normalize_runtime_defaults();
    Ok(node)
}

fn json_string_array(value: Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn json_uuid_array(value: Value) -> Vec<Uuid> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|value| Uuid::parse_str(value).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn vector_literal(vector: &[f32]) -> String {
    let values = vector
        .iter()
        .map(|value| {
            if value.is_finite() {
                value.to_string()
            } else {
                "0".to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{values}]")
}

fn rank_keyword_candidates(
    query: &str,
    candidates: Vec<MemoryNode>,
    opts: RetrievalOpts,
) -> Vec<MemoryNode> {
    let min_similarity = opts.min_similarity.unwrap_or(0.0).clamp(0.0, 1.0);
    let mut scored = candidates
        .into_iter()
        .map(|memory| {
            let score = AliaMemory::score_relevance(query, &memory);
            (memory, score)
        })
        .filter(|(_, score)| *score >= min_similarity && *score > 0.0)
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
        .into_iter()
        .take(opts.max_results.unwrap_or(10).clamp(1, 50) as usize)
        .map(|(memory, score)| {
            let mut memory = memory;
            memory.metadata = merge_retrieval_metadata(
                memory.metadata,
                RetrievalScoreBreakdown {
                    retrieval_path: "keyword_metadata".to_string(),
                    semantic_similarity: score,
                    recency_score: 0.0,
                    salience: memory.salience,
                    importance: memory.importance,
                    graph_relevance: 0.0,
                    confidence: memory.confidence,
                    affect_relevance: if memory.affect_tags.is_empty() {
                        0.0
                    } else {
                        0.5
                    },
                    final_score: score,
                    reason: "Keyword and metadata overlap matched the retrieval query.".to_string(),
                },
            );
            memory
        })
        .collect()
}

#[derive(Debug, Clone)]
struct RetrievalScoreBreakdown {
    retrieval_path: String,
    semantic_similarity: f32,
    recency_score: f32,
    salience: f32,
    importance: f32,
    graph_relevance: f32,
    confidence: f32,
    affect_relevance: f32,
    final_score: f32,
    reason: String,
}

fn merge_retrieval_metadata(metadata: Value, score: RetrievalScoreBreakdown) -> Value {
    let mut metadata = if metadata.is_object() {
        metadata
    } else {
        serde_json::json!({})
    };
    if let Some(object) = metadata.as_object_mut() {
        object.insert(
            "retrieval_path".to_string(),
            serde_json::json!(score.retrieval_path),
        );
        object.insert(
            "semantic_similarity".to_string(),
            serde_json::json!(score.semantic_similarity),
        );
        object.insert(
            "recency_score".to_string(),
            serde_json::json!(score.recency_score),
        );
        object.insert("salience".to_string(), serde_json::json!(score.salience));
        object.insert(
            "importance".to_string(),
            serde_json::json!(score.importance),
        );
        object.insert(
            "graph_relevance".to_string(),
            serde_json::json!(score.graph_relevance),
        );
        object.insert(
            "confidence".to_string(),
            serde_json::json!(score.confidence),
        );
        object.insert(
            "affect_relevance".to_string(),
            serde_json::json!(score.affect_relevance),
        );
        object.insert(
            "retrieval_score".to_string(),
            serde_json::json!(score.final_score),
        );
        object.insert(
            "retrieval_reason".to_string(),
            serde_json::json!(score.reason),
        );
    }
    metadata
}

fn merge_evidence_metadata(metadata: Value, confidence: f32, importance: f32) -> Value {
    let mut metadata = if metadata.is_object() {
        metadata
    } else {
        serde_json::json!({})
    };
    if let Some(object) = metadata.as_object_mut() {
        object.insert(
            "last_reinforced_confidence".to_string(),
            serde_json::json!(confidence),
        );
        object.insert(
            "last_reinforced_importance".to_string(),
            serde_json::json!(importance),
        );
    }
    metadata
}

fn prepare_memory_for_runtime(node: &mut MemoryNode, storage: &str) {
    if node.topic_tags.is_empty() {
        node.topic_tags = inferred_topic_tags(node);
    }
    node.topic_tags.sort();
    node.topic_tags.dedup();
    node.evidence = normalize_memory_evidence(node.evidence.clone(), node, storage);
}

fn normalize_memory_evidence(mut evidence: Value, node: &MemoryNode, storage: &str) -> Value {
    if !evidence.is_object() {
        evidence = serde_json::json!({});
    }
    if let Some(object) = evidence.as_object_mut() {
        object
            .entry("source".to_string())
            .or_insert_with(|| serde_json::json!("memory_runtime"));
        object.entry("summary".to_string()).or_insert_with(|| {
            serde_json::json!(format!(
                "Stored {} memory through the Alia Memory Runtime.",
                node.kind
            ))
        });
        object
            .entry("extractor".to_string())
            .or_insert_with(|| serde_json::json!("alia_memory_runtime_v1"));
        object
            .entry("policy_result".to_string())
            .or_insert_with(|| serde_json::json!("accepted"));
        object
            .entry("reason".to_string())
            .or_insert_with(|| serde_json::json!("Memory passed policy and durability checks."));
        object
            .entry("confidence_basis".to_string())
            .or_insert_with(|| {
                serde_json::json!({
                    "confidence": node.confidence,
                    "importance": node.importance,
                    "salience": node.salience,
                })
            });
        object
            .entry("evidence_count".to_string())
            .or_insert_with(|| serde_json::json!(1));
        object.insert("storage".to_string(), serde_json::json!(storage));
    }
    evidence
}

fn inferred_topic_tags(node: &MemoryNode) -> Vec<String> {
    let mut topics = metadata_string_list(&node.metadata, "topics")
        .into_iter()
        .chain(content_topic_candidates(&node.content))
        .map(|topic| normalize_graph_key(&topic))
        .filter(|topic| !topic.is_empty())
        .collect::<Vec<_>>();
    match node.kind.as_str() {
        "preference" => topics.push("preference".to_string()),
        "boundary" => topics.push("boundary".to_string()),
        "project" => topics.push("project".to_string()),
        "affective_pattern" => topics.push("affective_guidance".to_string()),
        _ => {}
    }
    topics.sort();
    topics.dedup();
    topics.truncate(8);
    topics
}

fn content_topic_candidates(content: &str) -> Vec<String> {
    let lower = content.to_ascii_lowercase();
    let mut topics = Vec::new();
    for (needle, topic) in [
        ("memory", "memory"),
        ("graph", "graph"),
        ("expression", "expression"),
        ("motion", "motion"),
        ("voice", "voice"),
        ("tts", "tts"),
        ("stt", "stt"),
        ("privacy", "privacy"),
        ("project", "project"),
        ("api", "api"),
        ("database", "database"),
        ("postgres", "postgres"),
        ("device", "device"),
        ("sensor", "sensor"),
        ("prompt", "prompt"),
        ("architecture", "architecture"),
        ("情感", "affect"),
        ("记忆", "memory"),
        ("表达", "expression"),
    ] {
        if lower.contains(needle) {
            topics.push(topic.to_string());
        }
    }
    topics
}

fn memory_merge_key(content: &str) -> String {
    crate::memory_policy::normalize_for_match(content)
}

fn normalized_topic_tags(node: &MemoryNode) -> Vec<String> {
    let mut tags = node
        .topic_tags
        .iter()
        .map(|tag| normalize_graph_key(tag))
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    if tags.is_empty() {
        tags.extend(
            metadata_string_list(&node.metadata, "topics")
                .into_iter()
                .map(|tag| normalize_graph_key(&tag))
                .filter(|tag| !tag.is_empty()),
        );
    }
    tags.sort();
    tags.dedup();
    tags
}

fn metadata_string_list(metadata: &Value, key: &str) -> Vec<String> {
    metadata
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn graph_entities_for_memory(node: &MemoryNode) -> Vec<String> {
    let mut entities = metadata_string_list(&node.metadata, "entities");
    entities.extend(extract_capitalized_entities(&node.content));
    entities.extend(
        normalized_topic_tags(node)
            .into_iter()
            .filter(|topic| {
                matches!(
                    topic.as_str(),
                    "memory" | "expression" | "affect" | "tts" | "stt"
                )
            })
            .map(|topic| format!("{topic} runtime")),
    );
    entities.sort();
    entities.dedup();
    entities.truncate(12);
    entities
}

fn graph_devices_for_memory(node: &MemoryNode) -> Vec<String> {
    let mut devices = metadata_string_list(&node.metadata, "devices");
    let lower = node.content.to_ascii_lowercase();
    for (needle, device) in [
        ("speaker", "speaker"),
        ("microphone", "microphone"),
        ("camera", "camera"),
        ("motion", "motion adapter"),
        ("light", "light"),
        ("screen", "display"),
        ("embedded", "embedded device"),
    ] {
        if lower.contains(needle) {
            devices.push(device.to_string());
        }
    }
    devices.sort();
    devices.dedup();
    devices
}

fn extract_capitalized_entities(content: &str) -> Vec<String> {
    let mut entities = Vec::new();
    let words = content
        .split_whitespace()
        .map(|word| word.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '-'))
        .filter(|word| word.len() > 2)
        .collect::<Vec<_>>();
    let mut current = Vec::new();
    for word in words {
        let starts_entity = word.chars().next().is_some_and(|ch| ch.is_uppercase())
            || word.chars().any(|ch| ch == '-')
            || word.chars().all(|ch| ch.is_ascii_uppercase());
        if starts_entity {
            current.push(word.to_string());
        } else if !current.is_empty() {
            entities.push(current.join(" "));
            current.clear();
        }
    }
    if !current.is_empty() {
        entities.push(current.join(" "));
    }
    entities
        .into_iter()
        .filter(|entity| !matches!(entity.as_str(), "The" | "User" | "Assistant"))
        .take(8)
        .collect()
}

fn normalize_graph_key(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_")
}

fn preference_is_dislike(content: &str) -> bool {
    let normalized = content.to_ascii_lowercase();
    normalized.contains("dislike")
        || normalized.contains("does not like")
        || normalized.contains("don't like")
        || normalized.contains("do not like")
        || normalized.contains("avoid")
        || normalized.contains("prefers not")
}

fn preference_supersedes(new_content: &str, old_content: &str) -> bool {
    let new_key = memory_merge_key(new_content);
    let old_key = memory_merge_key(old_content);
    if new_key == old_key {
        return false;
    }
    let new_lower = new_content.to_ascii_lowercase();
    let old_lower = old_content.to_ascii_lowercase();
    let negates = new_lower.contains("no longer")
        || new_lower.contains("instead")
        || new_lower.contains("prefer not")
        || new_lower.contains("do not")
        || new_lower.contains("don't")
        || new_lower.contains("avoid");
    let polarity_changed = preference_is_dislike(new_content) != preference_is_dislike(old_content);
    negates || polarity_changed || old_lower.contains("instead")
}

fn memory_quality(memory: &MemoryNode) -> f32 {
    (0.40 * memory.confidence + 0.30 * memory.importance + 0.20 * memory.salience).clamp(0.0, 1.0)
}

fn consolidation_metadata(keeper: &MemoryNode, other: &MemoryNode, reason: &str) -> Value {
    let mut metadata = keeper.metadata.clone();
    if !metadata.is_object() {
        metadata = serde_json::json!({});
    }
    if let Some(object) = metadata.as_object_mut() {
        object.insert(
            "consolidation_reason".to_string(),
            serde_json::json!(reason),
        );
        object.insert(
            "consolidated_at".to_string(),
            serde_json::json!(chrono::Utc::now().to_rfc3339()),
        );
        if let Some(id) = &other.id {
            object.insert("related_memory_id".to_string(), serde_json::json!(id));
        }
        object.insert(
            "evidence_count".to_string(),
            serde_json::json!(
                keeper
                    .metadata
                    .get("evidence_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    + 1
            ),
        );
    }
    metadata
}

fn content_signals_user_preference(content: &str) -> bool {
    let content = content.to_ascii_lowercase();
    content.contains("prefer")
        || content.contains("like")
        || content.contains("dislike")
        || content.contains("want")
        || content.contains("avoid")
        || content.contains("do not")
        || content.contains("don't")
        || content.contains("likes")
        || content.contains("wants")
        || content.contains("偏好")
        || content.contains("喜欢")
        || content.contains("不喜欢")
        || content.contains("希望")
        || content.contains("避免")
}

fn constructive_preference_content(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.to_ascii_lowercase().starts_with("user ") {
        trimmed.to_string()
    } else {
        format!("User preference inferred from prior episode: {trimmed}")
    }
}

fn memory_topic_overlap(left: &MemoryNode, right: &MemoryNode) -> bool {
    let left_tags = normalized_topic_tags(left);
    let right_tags = normalized_topic_tags(right);
    if left_tags.is_empty() || right_tags.is_empty() {
        let left_key = memory_merge_key(&left.content);
        let right_content = right.content.to_ascii_lowercase();
        return left_key
            .split_whitespace()
            .take(8)
            .any(|word| word.len() > 4 && right_content.contains(word));
    }
    left_tags.iter().any(|tag| right_tags.contains(tag))
}

fn is_negative_affective_memory(content: &str) -> bool {
    let content = content.to_ascii_lowercase();
    content.contains("frustrat")
        || content.contains("angry")
        || content.contains("upset")
        || content.contains("annoy")
        || content.contains("negative")
        || content.contains("不满")
        || content.contains("生气")
        || content.contains("挫败")
}

fn constructive_affective_guidance(content: &str) -> String {
    let content = content.trim();
    if content
        .to_ascii_lowercase()
        .starts_with("constructive guidance:")
    {
        content.to_string()
    } else {
        format!(
            "Constructive guidance: treat this as a signal to be clearer, calmer, and more specific. Evidence: {content}"
        )
    }
}

fn hidden_memory_export_reason(memory: &MemoryNode) -> Option<String> {
    if memory.visibility == "private_runtime" {
        Some("private_runtime_memory_hidden_from_user_export".to_string())
    } else if memory.sensitive && memory.visibility != "user_visible" {
        Some("sensitive_internal_memory_hidden_from_user_export".to_string())
    } else {
        None
    }
}

async fn upsert_graph_node(
    db: &(impl ConnectionTrait + Sync),
    user_id: Uuid,
    node_type: &str,
    node_key: &str,
    label: &str,
    properties: Value,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    let row = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO memory_nodes
                (id, tenant_id, user_id, node_type, node_key, label, properties, updated_at)
            VALUES
                ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6, $7::jsonb, NOW())
            ON CONFLICT (tenant_id, user_id, node_type, node_key) DO UPDATE SET
                label = EXCLUDED.label,
                properties = memory_nodes.properties || EXCLUDED.properties,
                updated_at = NOW()
            RETURNING id::text
            "#,
            vec![
                id.to_string().into(),
                DEFAULT_TENANT_ID.to_string().into(),
                user_id.to_string().into(),
                node_type.to_string().into(),
                node_key.to_string().into(),
                label.chars().take(512).collect::<String>().into(),
                properties.into(),
            ],
        ))
        .await?
        .ok_or_else(|| anyhow::anyhow!("memory graph node upsert returned no row"))?;
    let id: String = row.try_get("", "id")?;
    Ok(Uuid::parse_str(&id)?)
}

async fn upsert_graph_edge(
    db: &(impl ConnectionTrait + Sync),
    user_id: Uuid,
    source: Uuid,
    target: Uuid,
    relation: &str,
    confidence: f32,
    evidence_memory_id: Uuid,
) -> anyhow::Result<()> {
    db.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"
        INSERT INTO memory_edges
            (id, tenant_id, user_id, src_node_id, dst_node_id, relation,
             weight, confidence, evidence_memory_id, source_revision)
        VALUES
            ($1::uuid, $2::uuid, $3::uuid, $4::uuid, $5::uuid, $6, $7, $8, $9::uuid, (SELECT revision FROM memory_items WHERE id=$9::uuid))
        ON CONFLICT (tenant_id, user_id, src_node_id, dst_node_id, relation, evidence_memory_id)
        DO UPDATE SET
            weight = GREATEST(memory_edges.weight, EXCLUDED.weight),
            confidence = GREATEST(memory_edges.confidence, EXCLUDED.confidence),
            source_revision = EXCLUDED.source_revision, valid_to = NULL
        "#,
        vec![
            Uuid::new_v4().to_string().into(),
            DEFAULT_TENANT_ID.to_string().into(),
            user_id.to_string().into(),
            source.to_string().into(),
            target.to_string().into(),
            relation.to_string().into(),
            confidence.clamp(0.0, 1.0).into(),
            confidence.clamp(0.0, 1.0).into(),
            evidence_memory_id.to_string().into(),
        ],
    ))
    .await?;
    Ok(())
}
