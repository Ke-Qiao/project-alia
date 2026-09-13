//! Shared canonical retrieval for standalone Memory and caller-owned snapshots.
use super::*;

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
    profile: &str,
    vector: Option<&[f32]>,
) -> anyhow::Result<Vec<MemoryNode>> {
    if let Some(vector) = vector.filter(|v| v.len() == 1536 && v.iter().all(|x| x.is_finite())) {
        let sql = format!(
            "WITH ranked AS (SELECT {COLUMNS},1-(e.embedding <=> $3::vector) AS semantic_similarity,GREATEST(0.0,1.0-EXTRACT(EPOCH FROM (NOW()-m.updated_at))/2592000.0)::real AS recency_score,LEAST(1.0,COALESCE(es.n,0)::real/3.0)::real AS graph_relevance,CASE WHEN cardinality(m.affect_tags)>0 THEN 0.5::real ELSE 0.0::real END AS affect_relevance FROM memory_items m JOIN memory_embeddings_1536 e ON e.memory_id=m.id AND e.source_revision=m.revision LEFT JOIN LATERAL(SELECT COUNT(*) AS n FROM memory_live_edges x WHERE x.evidence_memory_id=m.id AND x.user_id=m.user_id AND x.tenant_id=m.tenant_id) es ON TRUE WHERE m.tenant_id=$1 AND m.user_id=$2 AND e.profile_id=$4 AND m.status='active' AND ($5::text[] IS NULL OR m.memory_layer=ANY($5)) AND (m.expires_at IS NULL OR m.expires_at>clock_timestamp())) SELECT *, (0.40*semantic_similarity+0.15*recency_score+0.15*salience+0.10*importance+0.10*graph_relevance+0.05*confidence+0.05*affect_relevance)::real AS final_score FROM ranked ORDER BY final_score DESC,semantic_similarity DESC,id LIMIT $6"
        );
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                &sql,
                vec![
                    DEFAULT_TENANT_ID.into(),
                    user_id.into(),
                    vector_literal(vector).into(),
                    profile.into(),
                    opts.layers.clone().into(),
                    opts.max_results.unwrap_or(10).clamp(1, 50).into(),
                ],
            ))
            .await?;
        let mut nodes = Vec::new();
        for row in rows {
            let similarity: f64 = row.try_get("", "semantic_similarity")?;
            if similarity < f64::from(opts.min_similarity.unwrap_or(0.0).clamp(0.0, 1.0)) {
                continue;
            }
            let score = RetrievalScoreBreakdown {
                retrieval_path: "vector_hnsw".into(),
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
        "SELECT {COLUMNS} FROM memory_items m WHERE m.tenant_id=$1 AND m.user_id=$2 AND m.status='active' AND ($3::text[] IS NULL OR m.memory_layer=ANY($3)) AND (m.expires_at IS NULL OR m.expires_at>clock_timestamp()) ORDER BY m.importance DESC,m.salience DESC,m.updated_at DESC,m.id LIMIT 200"
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
    Ok(rank_keyword_candidates(query, nodes, opts))
}
