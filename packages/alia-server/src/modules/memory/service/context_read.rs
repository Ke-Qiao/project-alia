//! Shared canonical retrieval for standalone Memory and caller-owned snapshots.
use super::embedding::ProfiledMemoryQuery;
use super::*;
use crate::modules::memory::context::ContextPolicy;

pub(crate) fn narrow_opts(policy: &ContextPolicy, opts: RetrievalOpts) -> RetrievalOpts {
    let layers = match opts.layers {
        Some(mut layers) => {
            if !policy.retrieval_layers.is_empty() {
                layers.retain(|layer| policy.retrieval_layers.contains(layer));
            }
            layers.sort();
            layers.dedup();
            Some(layers)
        }
        None => (!policy.retrieval_layers.is_empty()).then(|| policy.retrieval_layers.clone()),
    };
    RetrievalOpts {
        max_results: Some(
            opts.max_results
                .unwrap_or(policy.max_memory_results as u32)
                .min(policy.max_memory_results as u32),
        ),
        min_similarity: Some(
            opts.min_similarity
                .unwrap_or(policy.similarity_threshold)
                .max(policy.similarity_threshold),
        ),
        layers,
    }
}

const COLUMNS: &str = "m.id::text,m.revision,m.kind,m.scope,m.memory_layer,m.node_type,m.content,m.summary,m.confidence,m.salience,m.importance,m.visibility,m.sensitive,m.status,to_jsonb(m.topic_tags) AS topic_tags,to_jsonb(m.affect_tags) AS affect_tags,to_jsonb(m.source_event_ids) AS source_event_ids,m.evidence,m.metadata,m.superseded_by::text,m.expires_at";

pub(crate) async fn context_node(
    db: &(impl ConnectionTrait + Sync),
    user: Uuid,
    id: Uuid,
) -> anyhow::Result<Option<MemoryNode>> {
    let row=db.query_one(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        format!("SELECT {COLUMNS} FROM memory_items m WHERE m.id=$1 AND m.user_id=$2 AND m.tenant_id=$3 AND m.status='active' AND NOT m.sensitive AND m.visibility<>'private_runtime' AND (m.expires_at IS NULL OR m.expires_at>clock_timestamp()) FOR SHARE"),
        vec![id.into(),user.into(),Uuid::nil().into()])).await?;
    row.map(row_to_memory_node).transpose()
}

pub(crate) async fn retrieve_context_nodes(
    db: &(impl ConnectionTrait + Sync),
    user_id: Uuid,
    query: &str,
    opts: RetrievalOpts,
    vector: Option<&ProfiledMemoryQuery>,
    policy: &ContextPolicy,
) -> anyhow::Result<Vec<MemoryNode>> {
    let opts = narrow_opts(policy, opts);
    if !policy.reads_memory()
        || opts.max_results == Some(0)
        || opts.layers.as_ref().is_some_and(Vec::is_empty)
    {
        return Ok(Vec::new());
    }
    if let Some(vector) = vector {
        anyhow::ensure!(
            policy.active_embedding_profile.as_deref() == Some(vector.profile.id()?.as_str()),
            "Memory query vector differs from persistent profile"
        );
        let sql = format!(
            "WITH matching AS MATERIALIZED (SELECT m.*,e.embedding,e.input_digest FROM memory_items m JOIN memory_embeddings e ON e.memory_id=m.id AND e.source_revision=m.revision JOIN embedding_profiles p ON p.id=e.profile_id WHERE m.tenant_id=$1 AND m.user_id=$2 AND e.profile_id=$4 AND e.dimensions=$5 AND p.dimensions=$5 AND p.request_descriptor=$6 AND m.status='active' AND NOT m.sensitive AND m.visibility<>'private_runtime' AND ($7::text[] IS NULL OR m.memory_layer=ANY($7)) AND (m.expires_at IS NULL OR m.expires_at>clock_timestamp())), ranked AS (SELECT {COLUMNS},m.input_digest,1-(m.embedding <=> $3::vector) AS semantic_similarity,GREATEST(0.0,1.0-EXTRACT(EPOCH FROM (NOW()-m.updated_at))/2592000.0)::real AS recency_score,LEAST(1.0,COALESCE(es.n,0)::real/3.0)::real AS graph_relevance,CASE WHEN cardinality(m.affect_tags)>0 THEN 0.5::real ELSE 0.0::real END AS affect_relevance FROM matching m LEFT JOIN LATERAL(SELECT COUNT(*) AS n FROM memory_live_edges x WHERE x.evidence_memory_id=m.id AND x.user_id=m.user_id AND x.tenant_id=m.tenant_id) es ON TRUE) SELECT *, (0.40*semantic_similarity+0.15*recency_score+0.15*salience+0.10*importance+0.10*graph_relevance+0.05*confidence+0.05*affect_relevance)::real AS final_score FROM ranked ORDER BY final_score DESC,semantic_similarity DESC,id LIMIT $8"
        );
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                &sql,
                vec![
                    DEFAULT_TENANT_ID.into(),
                    user_id.into(),
                    vector_literal(&vector.vector.values).into(),
                    vector.profile.id()?.into(),
                    (vector.profile.dimensions as i32).into(),
                    serde_json::to_value(&vector.profile)?.into(),
                    opts.layers.clone().into(),
                    opts.max_results.unwrap_or(10).clamp(1, 50).into(),
                ],
            ))
            .await?;
        let mut nodes = Vec::new();
        for row in rows {
            let content: String = row.try_get("", "content")?;
            anyhow::ensure!(
                row.try_get::<String>("", "input_digest")?
                    == crate::providers::adapter::embedding_text_digest(&content),
                "Memory vector input differs from its canonical revision"
            );
            if policy.sensitive_filter_enabled
                && crate::modules::memory::policy::detect_sensitive_memory(&content).is_some()
            {
                continue;
            }
            let similarity: f64 = row.try_get("", "semantic_similarity")?;
            if similarity < f64::from(opts.min_similarity.unwrap_or(0.0).clamp(0.0, 1.0)) {
                continue;
            }
            let score = RetrievalScoreBreakdown {
                retrieval_path: "profiled_vector".into(),
                semantic_similarity: similarity as f32,
                recency_score: row.try_get("", "recency_score")?,
                salience: row.try_get("", "salience")?,
                importance: row.try_get("", "importance")?,
                graph_relevance: row.try_get("", "graph_relevance")?,
                confidence: row.try_get("", "confidence")?,
                affect_relevance: row.try_get("", "affect_relevance")?,
                final_score: row.try_get("", "final_score")?,
                reason: "Vector similarity reranked with canonical memory signals.".into(),
            };
            let mut node = row_to_memory_node(row)?;
            node.metadata = merge_retrieval_metadata(node.metadata, score);
            nodes.push(node);
        }
        if !nodes.is_empty() {
            return Ok(nodes);
        }
    }
    let sql = format!(
        "SELECT {COLUMNS} FROM memory_items m WHERE m.tenant_id=$1 AND m.user_id=$2 AND m.status='active' AND NOT m.sensitive AND m.visibility<>'private_runtime' AND ($3::text[] IS NULL OR m.memory_layer=ANY($3)) AND (m.expires_at IS NULL OR m.expires_at>clock_timestamp()) ORDER BY m.importance DESC,m.salience DESC,m.updated_at DESC,m.id LIMIT 200"
    );
    let rows = db
        .query_all(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            &sql,
            vec![
                DEFAULT_TENANT_ID.into(),
                user_id.into(),
                opts.layers.clone().into(),
            ],
        ))
        .await?;
    let nodes = rows
        .into_iter()
        .map(row_to_memory_node)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let nodes = nodes
        .into_iter()
        .filter(|node| {
            !policy.sensitive_filter_enabled
                || crate::modules::memory::policy::detect_sensitive_memory(&node.content).is_none()
        })
        .collect();
    Ok(rank_keyword_candidates(query, nodes, opts))
}
