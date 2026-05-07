//! Hybrid Retrieval System
//!
//! Combines three search modes following GraphRAG methodology:
//! 1. Local Search - Entity-centric graph traversal
//! 2. Global Search - Community-level semantic search
//! 3. Vector Search - Direct embedding similarity
//!
//! Results are ranked by a combination of relevance and importance.

use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Statement,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::memory_node;
use crate::entity::memory_node::MemoryLayer;
use crate::providers::memory::graph::KnowledgeGraph;

/// Search result with relevance score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub node_id: Uuid,
    pub content: String,
    pub memory_layer: String,
    pub node_type: String,
    pub relevance_score: f32,
    pub importance: f32,
    pub metadata: serde_json::Value,
}

/// Community search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityResult {
    pub community_id: Uuid,
    pub title: String,
    pub summary: String,
    pub relevance_score: f32,
    pub node_count: usize,
}

/// Retrieval options
#[derive(Debug, Clone, Default)]
pub struct RetrievalOptions {
    /// Maximum results to return
    pub max_results: usize,
    /// Minimum relevance score (0.0 - 1.0)
    pub min_relevance: f32,
    /// Memory layers to search (None = all)
    pub layers: Option<Vec<String>>,
    /// Whether to include graph context (neighbors)
    pub include_graph_context: bool,
    /// Graph traversal depth for context
    pub graph_depth: u32,
}

impl RetrievalOptions {
    pub fn new() -> Self {
        Self {
            max_results: 10,
            min_relevance: 0.3,
            layers: None,
            include_graph_context: true,
            graph_depth: 1,
        }
    }
}

/// Hybrid retrieval engine
pub struct HybridRetrieval {
    db: DatabaseConnection,
    graph: KnowledgeGraph,
}

impl HybridRetrieval {
    pub fn new(db: DatabaseConnection) -> Self {
        let graph = KnowledgeGraph::new(db.clone());
        Self { db, graph }
    }

    /// Main retrieval entry point - combines all search modes
    pub async fn retrieve(
        &self,
        user_id: Uuid,
        query: &str,
        query_embedding: Option<&[f32]>,
        entities: &[String],
        opts: &RetrievalOptions,
    ) -> anyhow::Result<Vec<SearchResult>> {
        let mut all_results: Vec<SearchResult> = Vec::new();

        // 1. Local Search - Entity-centric
        if !entities.is_empty() {
            let local_results = self.local_search(user_id, entities, opts).await?;
            all_results.extend(local_results);
        }

        // 2. Vector Search - Semantic similarity
        if let Some(embedding) = query_embedding {
            let vector_results = self.vector_search(user_id, embedding, opts).await?;
            all_results.extend(vector_results);
        }

        // 3. Keyword fallback if no embedding
        if query_embedding.is_none() {
            let keyword_results = self.keyword_search(user_id, query, opts).await?;
            all_results.extend(keyword_results);
        }

        // Deduplicate by node_id, keeping highest relevance
        let mut seen: std::collections::HashMap<Uuid, usize> = std::collections::HashMap::new();
        let mut deduped: Vec<SearchResult> = Vec::new();

        for result in all_results {
            if let Some(&idx) = seen.get(&result.node_id) {
                if result.relevance_score > deduped[idx].relevance_score {
                    deduped[idx] = result;
                }
            } else {
                seen.insert(result.node_id, deduped.len());
                deduped.push(result);
            }
        }

        // Sort by combined score (relevance * 0.7 + importance * 0.3)
        deduped.sort_by(|a, b| {
            let score_a = a.relevance_score * 0.7 + a.importance * 0.3;
            let score_b = b.relevance_score * 0.7 + b.importance * 0.3;
            score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Limit results
        deduped.truncate(opts.max_results);

        Ok(deduped)
    }

    /// Local search: Find nodes matching entities and traverse their graph neighborhood
    pub async fn local_search(
        &self,
        user_id: Uuid,
        entities: &[String],
        opts: &RetrievalOptions,
    ) -> anyhow::Result<Vec<SearchResult>> {
        let mut results: Vec<SearchResult> = Vec::new();

        // Find nodes matching entity names
        for entity in entities {
            let pattern = format!("%{}%", entity);

            let nodes = memory_node::Entity::find()
                .filter(memory_node::Column::UserId.eq(user_id))
                .filter(memory_node::Column::Content.like(&pattern))
                .order_by_desc(memory_node::Column::Importance)
                .limit(5)
                .all(&self.db)
                .await?;

            for node in nodes {
                // Skip if not in allowed layers
                if let Some(ref layers) = opts.layers {
                    if !layers.contains(&node.memory_layer.to_string()) {
                        continue;
                    }
                }

                let relevance = self.calculate_keyword_relevance(&node.content, entity);
                if relevance < opts.min_relevance {
                    continue;
                }

                results.push(SearchResult {
                    node_id: node.id,
                    content: node.content.clone(),
                    memory_layer: node.memory_layer.to_string(),
                    node_type: node.node_type.to_string(),
                    relevance_score: relevance,
                    importance: node.importance,
                    metadata: node.metadata.clone(),
                });

                // Get graph context if requested
                if opts.include_graph_context && opts.graph_depth > 0 {
                    let fragment = self.graph.get_neighbors(node.id, opts.graph_depth).await?;
                    for neighbor in fragment.nodes {
                        if neighbor.id != node.id {
                            // Decay relevance by depth
                            let neighbor_relevance = relevance * 0.5;
                            if neighbor_relevance >= opts.min_relevance {
                                results.push(SearchResult {
                                    node_id: neighbor.id,
                                    content: neighbor.content,
                                    memory_layer: neighbor.memory_layer,
                                    node_type: neighbor.node_type,
                                    relevance_score: neighbor_relevance,
                                    importance: neighbor.importance,
                                    metadata: neighbor.metadata,
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    /// Vector search: Semantic similarity using pgvector
    pub async fn vector_search(
        &self,
        user_id: Uuid,
        query_embedding: &[f32],
        opts: &RetrievalOptions,
    ) -> anyhow::Result<Vec<SearchResult>> {
        // Build the vector string for pgvector
        let vector_str = format!(
            "[{}]",
            query_embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        // Build layer filter
        let layer_filter = if let Some(ref layers) = opts.layers {
            let layer_list = layers
                .iter()
                .map(|l| format!("'{}'", l))
                .collect::<Vec<_>>()
                .join(",");
            format!("AND memory_layer IN ({})", layer_list)
        } else {
            String::new()
        };

        // Use raw SQL for pgvector operations
        let sql = format!(
            r#"
            SELECT 
                id, content, memory_layer, node_type, importance, metadata,
                1 - (embedding <=> $1::vector) as similarity
            FROM memory_nodes
            WHERE user_id = $2
                AND embedding IS NOT NULL
                {}
            ORDER BY embedding <=> $1::vector
            LIMIT $3
            "#,
            layer_filter
        );

        let results = self
            .db
            .query_all(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                &sql,
                [
                    vector_str.into(),
                    user_id.to_string().into(),
                    (opts.max_results as i64).into(),
                ],
            ))
            .await?;

        let mut search_results: Vec<SearchResult> = Vec::new();

        for row in results {
            let similarity: f32 = row.try_get("", "similarity").unwrap_or(0.0);
            if similarity < opts.min_relevance {
                continue;
            }

            let id_str: String = row.try_get("", "id")?;
            let id = Uuid::parse_str(&id_str)?;

            search_results.push(SearchResult {
                node_id: id,
                content: row.try_get("", "content")?,
                memory_layer: row.try_get("", "memory_layer")?,
                node_type: row.try_get("", "node_type")?,
                relevance_score: similarity,
                importance: row.try_get("", "importance")?,
                metadata: row.try_get("", "metadata")?,
            });
        }

        Ok(search_results)
    }

    /// Global search: Search community summaries
    pub async fn global_search(
        &self,
        user_id: Uuid,
        query_embedding: &[f32],
        opts: &RetrievalOptions,
    ) -> anyhow::Result<Vec<CommunityResult>> {
        let vector_str = format!(
            "[{}]",
            query_embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let sql = r#"
            SELECT 
                id, title, summary, node_ids,
                1 - (embedding <=> $1::vector) as similarity
            FROM memory_communities
            WHERE user_id = $2
                AND embedding IS NOT NULL
            ORDER BY embedding <=> $1::vector
            LIMIT $3
        "#;

        let results = self
            .db
            .query_all(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                sql,
                [
                    vector_str.into(),
                    user_id.to_string().into(),
                    (opts.max_results as i64).into(),
                ],
            ))
            .await?;

        let mut community_results: Vec<CommunityResult> = Vec::new();

        for row in results {
            let similarity: f32 = row.try_get("", "similarity").unwrap_or(0.0);
            if similarity < opts.min_relevance {
                continue;
            }

            let id_str: String = row.try_get("", "id")?;
            let id = Uuid::parse_str(&id_str)?;
            let node_ids: serde_json::Value = row.try_get("", "node_ids")?;
            let node_count = node_ids.as_array().map_or(0, |a| a.len());

            community_results.push(CommunityResult {
                community_id: id,
                title: row.try_get("", "title")?,
                summary: row.try_get("", "summary")?,
                relevance_score: similarity,
                node_count,
            });
        }

        Ok(community_results)
    }

    /// Keyword search: Simple text matching fallback
    pub async fn keyword_search(
        &self,
        user_id: Uuid,
        query: &str,
        opts: &RetrievalOptions,
    ) -> anyhow::Result<Vec<SearchResult>> {
        let pattern = format!("%{}%", query);

        let mut query_builder = memory_node::Entity::find()
            .filter(memory_node::Column::UserId.eq(user_id))
            .filter(memory_node::Column::Content.like(&pattern));

        if let Some(ref layers) = opts.layers {
            let layer_enums: Vec<MemoryLayer> = layers
                .iter()
                .filter_map(|l| match l.as_str() {
                    "episodic" => Some(MemoryLayer::Episodic),
                    "core" => Some(MemoryLayer::Core),
                    "procedural" => Some(MemoryLayer::Procedural),
                    _ => None,
                })
                .collect();
            if !layer_enums.is_empty() {
                query_builder = query_builder.filter(memory_node::Column::MemoryLayer.is_in(layer_enums));
            }
        }

        let nodes = query_builder
            .order_by_desc(memory_node::Column::Importance)
            .limit(opts.max_results as u64)
            .all(&self.db)
            .await?;

        let results: Vec<SearchResult> = nodes
            .into_iter()
            .map(|node| {
                let relevance = self.calculate_keyword_relevance(&node.content, query);
                SearchResult {
                    node_id: node.id,
                    content: node.content,
                    memory_layer: node.memory_layer.to_string(),
                    node_type: node.node_type.to_string(),
                    relevance_score: relevance,
                    importance: node.importance,
                    metadata: node.metadata,
                }
            })
            .filter(|r| r.relevance_score >= opts.min_relevance)
            .collect();

        Ok(results)
    }

    /// Calculate simple keyword relevance score
    fn calculate_keyword_relevance(&self, content: &str, keyword: &str) -> f32 {
        let content_lower = content.to_lowercase();
        let keyword_lower = keyword.to_lowercase();

        if content_lower == keyword_lower {
            return 1.0;
        }

        if content_lower.contains(&keyword_lower) {
            // Score based on how much of the content is the keyword
            let ratio = keyword_lower.len() as f32 / content_lower.len() as f32;
            return (0.5 + ratio * 0.5).min(0.95);
        }

        // Check for partial word matches
        let content_words: Vec<&str> = content_lower.split_whitespace().collect();
        let keyword_words: Vec<&str> = keyword_lower.split_whitespace().collect();

        let mut matches = 0;
        for kw in &keyword_words {
            if content_words.iter().any(|w| w.contains(kw)) {
                matches += 1;
            }
        }

        if matches > 0 {
            return matches as f32 / keyword_words.len() as f32 * 0.5;
        }

        0.0
    }
}
