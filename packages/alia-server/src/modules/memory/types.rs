//! Canonical Memory domain values and retrieval projections.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryNode {
    #[serde(default)]
    pub revision: i64,
    pub id: Option<String>,
    pub memory_layer: String,
    pub node_type: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub importance: f32,
    #[serde(default = "default_memory_kind")]
    pub kind: String,
    #[serde(default = "default_memory_scope")]
    pub scope: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default = "default_memory_confidence")]
    pub confidence: f32,
    #[serde(default = "default_memory_salience")]
    pub salience: f32,
    #[serde(default = "default_memory_visibility")]
    pub visibility: String,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default = "default_memory_status")]
    pub status: String,
    #[serde(default)]
    pub topic_tags: Vec<String>,
    #[serde(default)]
    pub affect_tags: Vec<String>,
    #[serde(default)]
    pub evidence: serde_json::Value,
    #[serde(default)]
    pub source_event_ids: Vec<Uuid>,
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn default_memory_kind() -> String {
    "episodic".to_string()
}

fn default_memory_scope() -> String {
    "user".to_string()
}

fn default_memory_confidence() -> f32 {
    0.5
}

fn default_memory_salience() -> f32 {
    0.5
}

fn default_memory_visibility() -> String {
    "internal".to_string()
}

fn default_memory_status() -> String {
    "active".to_string()
}

impl MemoryNode {
    pub fn new(
        memory_layer: impl Into<String>,
        node_type: impl Into<String>,
        content: impl Into<String>,
        importance: f32,
    ) -> Self {
        let memory_layer = memory_layer.into();
        let node_type = node_type.into();
        let kind = normalize_runtime_memory_kind(&memory_layer, &node_type);
        Self {
            revision: 0,
            id: None,
            memory_layer,
            node_type,
            content: content.into(),
            metadata: serde_json::json!({}),
            importance: importance.clamp(0.0, 1.0),
            kind,
            scope: default_memory_scope(),
            summary: None,
            confidence: default_memory_confidence(),
            salience: default_memory_salience(),
            visibility: default_memory_visibility(),
            sensitive: false,
            status: default_memory_status(),
            topic_tags: Vec::new(),
            affect_tags: Vec::new(),
            evidence: serde_json::json!({}),
            source_event_ids: Vec::new(),
            superseded_by: None,
            expires_at: None,
        }
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn with_runtime_fields(
        mut self,
        kind: impl Into<String>,
        scope: impl Into<String>,
        confidence: f32,
        salience: f32,
        visibility: impl Into<String>,
    ) -> Self {
        self.kind = normalize_memory_kind_text(&kind.into());
        self.scope = normalize_memory_scope_text(&scope.into());
        self.confidence = confidence.clamp(0.0, 1.0);
        self.salience = salience.clamp(0.0, 1.0);
        self.visibility = normalize_memory_visibility_text(&visibility.into());
        self
    }

    pub fn normalize_runtime_defaults(&mut self) {
        self.memory_layer = normalize_memory_layer_text(&self.memory_layer);
        self.node_type = normalize_memory_node_type_text(&self.node_type);
        self.kind = if self.kind.trim().is_empty() {
            normalize_runtime_memory_kind(&self.memory_layer, &self.node_type)
        } else {
            normalize_memory_kind_text(&self.kind)
        };
        self.scope = normalize_memory_scope_text(&self.scope);
        self.visibility = normalize_memory_visibility_text(&self.visibility);
        self.status = normalize_memory_status_text(&self.status);
        self.importance = self.importance.clamp(0.0, 1.0);
        self.confidence = self.confidence.clamp(0.0, 1.0);
        self.salience = self.salience.clamp(0.0, 1.0);
    }
}

fn normalize_runtime_memory_kind(memory_layer: &str, node_type: &str) -> String {
    let layer = memory_layer.trim().to_ascii_lowercase();
    let node_type = node_type.trim().to_ascii_lowercase();
    match (layer.as_str(), node_type.as_str()) {
        ("preference", _) | (_, "preference") | (_, "habit") => "preference",
        ("relationship", _) | (_, "relationship") => "social",
        ("reflection", "insight") => "affective_pattern",
        ("project", _) | (_, "goal") => "project",
        ("boundary", _) | (_, "boundary") => "boundary",
        ("core", "fact") => "semantic",
        ("episodic", _) | (_, "event") => "episodic",
        _ => "episodic",
    }
    .to_string()
}

fn normalize_memory_kind_text(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "episodic" | "semantic" | "preference" | "social" | "affective_pattern" | "system"
        | "project" | "boundary" => value.trim().to_ascii_lowercase(),
        "relationship" => "social".to_string(),
        "core" | "fact" => "semantic".to_string(),
        _ => "episodic".to_string(),
    }
}

fn normalize_memory_scope_text(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "turn" | "session" | "user" | "global" | "device" => value.trim().to_ascii_lowercase(),
        "relationship" => "user".to_string(),
        _ => "user".to_string(),
    }
}

fn normalize_memory_visibility_text(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "user_visible" | "internal" | "private_runtime" => value.trim().to_ascii_lowercase(),
        _ => "internal".to_string(),
    }
}

fn normalize_memory_status_text(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "active" | "superseded" | "rejected" | "expired" | "deleted" => {
            value.trim().to_ascii_lowercase()
        }
        _ => "active".to_string(),
    }
}

fn normalize_memory_layer_text(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "core" | "preference" | "episodic" | "relationship" | "reflection" | "project"
        | "boundary" => value.trim().to_ascii_lowercase(),
        "semantic" => "core".to_string(),
        "social" => "relationship".to_string(),
        _ => "episodic".to_string(),
    }
}

fn normalize_memory_node_type_text(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "fact" | "preference" | "relationship" | "goal" | "habit" | "event" | "insight"
        | "boundary" | "project" => value.trim().to_ascii_lowercase(),
        _ => "fact".to_string(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct RetrievalOpts {
    pub max_results: Option<u32>,
    pub min_similarity: Option<f32>,
    pub layers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryListOpts {
    pub max_results: Option<u32>,
    pub layers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryPatch {
    #[serde(default)]
    pub expected_revision: Option<i64>,
    pub content: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub importance: Option<f32>,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub salience: Option<f32>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryCapabilities {
    pub system: String,
    pub abi_version: u32,
    pub supports_store: bool,
    pub supports_list: bool,
    pub supports_get: bool,
    pub supports_retrieve: bool,
    pub supports_update: bool,
    pub supports_delete: bool,
    pub supports_clear_user: bool,
    pub supports_metadata: bool,
    pub supports_search_filters: bool,
    pub supports_layer_filters: bool,
    pub supports_similarity_threshold: bool,
    pub supports_graph: bool,
    pub supports_relationship_edges: bool,
    pub async_writes: bool,
    pub max_content_chars: Option<usize>,
    pub max_metadata_bytes: Option<usize>,
    pub max_batch_messages: Option<usize>,
    #[serde(default)]
    pub diagnostics: BTreeMap<String, serde_json::Value>,
}

impl MemoryCapabilities {
    pub fn minimal(system: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            abi_version: 1,
            supports_store: true,
            supports_list: true,
            supports_get: true,
            supports_retrieve: true,
            supports_update: true,
            supports_delete: true,
            supports_clear_user: true,
            supports_metadata: true,
            supports_search_filters: false,
            supports_layer_filters: true,
            supports_similarity_threshold: false,
            supports_graph: false,
            supports_relationship_edges: false,
            async_writes: false,
            max_content_chars: None,
            max_metadata_bytes: None,
            max_batch_messages: None,
            diagnostics: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphFragment {
    pub nodes: Vec<MemoryNode>,
    pub edges: Vec<MemoryEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEdge {
    pub source_id: String,
    pub target_id: String,
    pub relation_type: String,
    pub weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySnippet {
    pub id: String,
    pub kind: String,
    pub content: String,
    pub confidence: f32,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphSnippet {
    pub source: String,
    pub relation: String,
    pub target: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuppressedMemory {
    pub memory_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRetrievalTrace {
    pub memory_id: String,
    pub retrieval_path: String,
    pub semantic_similarity: f32,
    pub recency_score: f32,
    pub salience: f32,
    pub importance: f32,
    pub graph_relevance: f32,
    pub confidence: f32,
    pub affect_relevance: f32,
    pub final_score: f32,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryContextPack {
    pub user_preferences: Vec<MemorySnippet>,
    pub project_context: Vec<MemorySnippet>,
    pub relevant_episodes: Vec<MemorySnippet>,
    pub affective_guidance: Vec<MemorySnippet>,
    pub boundaries: Vec<MemorySnippet>,
    pub graph_context: Vec<GraphSnippet>,
    pub suppressed: Vec<SuppressedMemory>,
    pub recall_events: Vec<MemoryRecallEvent>,
    pub retrieval_trace: Vec<MemoryRetrievalTrace>,
}

impl MemoryContextPack {
    pub fn is_empty(&self) -> bool {
        self.user_preferences.is_empty()
            && self.project_context.is_empty()
            && self.relevant_episodes.is_empty()
            && self.affective_guidance.is_empty()
            && self.boundaries.is_empty()
            && self.graph_context.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecallEvent {
    pub memory_id: String,
    pub memory_kind: String,
    pub content: String,
    pub confidence: f32,
    pub salience: f32,
    pub affect_hint: MemoryAffectHint,
    pub evidence_memory_ids: Vec<String>,
    pub source_event_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryAffectHint {
    pub valence_delta: Option<f32>,
    pub arousal_delta: Option<f32>,
    pub trust_delta: Option<f32>,
    pub engagement_delta: Option<f32>,
    pub frustration_risk_delta: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryExplanation {
    pub memory: MemoryNode,
    pub why_exists: String,
    pub evidence_summary: String,
    pub source_event_ids: Vec<Uuid>,
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryConsolidationResult {
    pub scanned: usize,
    pub deduplicated: usize,
    pub promoted: usize,
    pub demoted: usize,
    pub superseded: usize,
    pub contradictions: usize,
    pub transformed_affective_guidance: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryExport {
    pub memories: Vec<MemoryNode>,
    pub suppressed: Vec<SuppressedMemory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySuppressedExplanation {
    pub memory_id: String,
    pub suppressed: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryWriteOrigin {
    Manual,
    Automatic,
    Ai,
    Maintenance,
}

#[derive(Debug, Clone)]
pub struct MemoryWriteContext {
    pub user_id: Uuid,
    pub privacy_epoch: i64,
    /// Independent relationship authority captured alongside privacy. Pure
    /// in-memory fixtures and legacy snapshots have no persistent authority.
    pub relationship_epoch: Option<i64>,
    /// Persisted policy captured before reading or deriving a write. In-memory
    /// fixtures have no authority to create a database policy snapshot.
    pub policy_version: Option<i64>,
    /// Immutable lineage of any model-derived content included in this write.
    pub context_id: Option<Uuid>,
    pub origin: MemoryWriteOrigin,
    pub source_cutoff_at: chrono::DateTime<chrono::Utc>,
    /// Trusted input timestamp, never an extractor-generated timestamp.
    pub source_received_at: chrono::DateTime<chrono::Utc>,
    pub source_event_ids: Vec<Uuid>,
    /// Optional audit consistency for a tool's single original user message.
    pub exact_source_received_at: Option<(Uuid, chrono::DateTime<chrono::Utc>)>,
    pub source_memories: Vec<(String, i64)>,
}

/// A consolidation plan against one immutable source snapshot. Retirement is
/// represented by a superseded/rejected update; this API never performs forget.
#[derive(Debug, Clone, Default)]
pub struct MemoryBatch {
    pub updates: Vec<(String, MemoryPatch)>,
    pub creates: Vec<MemoryNode>,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryBatchResult {
    pub updated: usize,
    pub created_ids: Vec<String>,
}

pub fn memory_context_pack_from_nodes(
    nodes: Vec<MemoryNode>,
    graph_context: Vec<GraphSnippet>,
) -> MemoryContextPack {
    let mut pack = MemoryContextPack {
        graph_context,
        ..MemoryContextPack::default()
    };
    for node in nodes {
        let id = node.id.clone().unwrap_or_default();
        if node.sensitive || node.visibility == "private_runtime" {
            pack.suppressed.push(SuppressedMemory {
                memory_id: id,
                reason: "sensitive_or_private_runtime".to_string(),
            });
            continue;
        }
        let snippet = MemorySnippet {
            id: id.clone(),
            kind: node.kind.clone(),
            content: constructive_memory_content(&node),
            confidence: node.confidence,
            source: node
                .metadata
                .get("source")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("memory_runtime")
                .to_string(),
        };
        match node.kind.as_str() {
            "preference" | "social" => pack.user_preferences.push(snippet),
            "project" | "semantic" => pack.project_context.push(snippet),
            "affective_pattern" => pack.affective_guidance.push(snippet),
            "boundary" => pack.boundaries.push(snippet),
            _ => pack.relevant_episodes.push(snippet),
        }
        if let Some(trace) = memory_retrieval_trace_from_node(&node) {
            pack.retrieval_trace.push(trace);
        }
        pack.recall_events.push(MemoryRecallEvent {
            memory_id: id.clone(),
            memory_kind: node.kind.clone(),
            content: node.content.clone(),
            confidence: node.confidence,
            salience: node.salience,
            affect_hint: bounded_affect_hint_for_memory(&node),
            evidence_memory_ids: vec![id],
            source_event_ids: node.source_event_ids.clone(),
        });
    }
    pack
}

fn memory_retrieval_trace_from_node(node: &MemoryNode) -> Option<MemoryRetrievalTrace> {
    let metadata = node.metadata.as_object()?;
    let memory_id = node.id.clone().unwrap_or_default();
    Some(MemoryRetrievalTrace {
        memory_id,
        retrieval_path: metadata
            .get("retrieval_path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        semantic_similarity: metadata
            .get("semantic_similarity")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0) as f32,
        recency_score: metadata
            .get("recency_score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0) as f32,
        salience: node.salience,
        importance: node.importance,
        graph_relevance: metadata
            .get("graph_relevance")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0) as f32,
        confidence: node.confidence,
        affect_relevance: metadata
            .get("affect_relevance")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0) as f32,
        final_score: metadata
            .get("retrieval_score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0) as f32,
        reason: metadata
            .get("retrieval_reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Memory matched the retrieval query.")
            .to_string(),
    })
}

fn constructive_memory_content(node: &MemoryNode) -> String {
    if node.kind == "affective_pattern" {
        format!(
            "Use this as constructive interaction guidance: {}",
            node.content.trim()
        )
    } else {
        node.content.clone()
    }
}

fn bounded_affect_hint_for_memory(node: &MemoryNode) -> MemoryAffectHint {
    let confidence = node.confidence.clamp(0.0, 1.0);
    let salience = node.salience.clamp(0.0, 1.0);
    let scale = confidence * salience;
    match node.kind.as_str() {
        "preference" | "boundary" => MemoryAffectHint {
            trust_delta: Some((0.02 * scale).min(0.02)),
            engagement_delta: Some((0.05 * scale).min(0.08)),
            ..MemoryAffectHint::default()
        },
        "affective_pattern" => MemoryAffectHint {
            valence_delta: Some((0.03 * scale).min(0.05)),
            engagement_delta: Some((0.04 * scale).min(0.08)),
            frustration_risk_delta: Some((-0.04 * scale).max(-0.08)),
            ..MemoryAffectHint::default()
        },
        _ => MemoryAffectHint {
            engagement_delta: Some((0.03 * scale).min(0.08)),
            ..MemoryAffectHint::default()
        },
    }
}
