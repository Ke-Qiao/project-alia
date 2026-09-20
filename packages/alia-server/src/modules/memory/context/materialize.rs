use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction, IsolationLevel, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{repository::*, *};
use crate::providers::adapter::{
    ChatMessage, GraphSnippet, RetrievalOpts, memory_context_pack_from_nodes,
};

impl ContextService {
    pub async fn materialize(
        &self,
        source: ContextSource,
        query: &str,
    ) -> ContextResult<GenerationContext> {
        self.materialize_inner(source, query, None, None).await
    }
    pub async fn extend_memory(
        &self,
        original: &GenerationContext,
        query: &str,
    ) -> ContextResult<GenerationContext> {
        self.materialize_inner(original.source().clone(), query, Some(original), None)
            .await
    }
    pub async fn extend_memory_with_opts(
        &self,
        original: &GenerationContext,
        query: &str,
        opts: RetrievalOpts,
    ) -> ContextResult<GenerationContext> {
        self.materialize_inner(original.source().clone(), query, Some(original), Some(opts))
            .await
    }
    async fn materialize_inner(
        &self,
        source: ContextSource,
        query: &str,
        original: Option<&GenerationContext>,
        override_opts: Option<RetrievalOpts>,
    ) -> ContextResult<GenerationContext> {
        anyhow::ensure!(
            source.source_event_id.is_some() && source.conversation_id.is_some(),
            "generation requires a real persisted user source"
        );
        if query.len() > MAX_CONTEXT_BYTES {
            return Err(ContextError::BoundsExceeded.into());
        }
        let policy = self.policy_store().current().await?;
        if original.is_some_and(|old| old.lease.policy_version != policy.version) {
            return Err(ContextError::PolicyChanged.into());
        }
        let mut reads_memory = policy.policy.reads_memory();
        let mut retrieval = RetrievalOpts {
            max_results: Some(policy.policy.max_memory_results as u32),
            min_similarity: Some(policy.policy.similarity_threshold),
            layers: (!policy.policy.retrieval_layers.is_empty())
                .then(|| policy.policy.retrieval_layers.clone()),
        };
        if let Some(opts) = override_opts {
            if let Some(max) = opts.max_results {
                anyhow::ensure!((1..=20).contains(&max), "invalid MemorySearch result bound");
                retrieval.max_results = Some(max.min(policy.policy.max_memory_results as u32));
            }
            if let Some(min) = opts.min_similarity {
                anyhow::ensure!(
                    min.is_finite() && (0.0..=1.0).contains(&min),
                    "invalid MemorySearch similarity threshold"
                );
                retrieval.min_similarity = Some(min.max(policy.policy.similarity_threshold));
            }
            if let Some(mut layers) = opts.layers {
                anyhow::ensure!(
                    layers.len() <= 16
                        && layers
                            .iter()
                            .all(|layer| !layer.is_empty() && layer.len() <= 64),
                    "invalid MemorySearch layers"
                );
                if !policy.policy.retrieval_layers.is_empty() {
                    layers.retain(|layer| policy.policy.retrieval_layers.contains(layer));
                }
                layers.sort();
                layers.dedup();
                reads_memory &= !layers.is_empty();
                retrieval.layers = Some(layers);
            }
        }
        // Admit the actual source and all original roots before disclosing a
        // query to the provider. No network work holds these locks; the same
        // authority is checked again in the materialization snapshot below.
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let tx = self.db.begin().await?;
            tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
                .await?;
            validate_materialization_source(&tx, &source, query, original, policy.version).await?;
            tx.commit().await?;
            Ok::<(), anyhow::Error>(())
        })
        .await
        .map_err(|_| ContextError::Unavailable("context query admission timed out".into()))??;
        let vector = if reads_memory {
            crate::providers::memory::simple::embedding::query_for_policy(
                &self.db,
                self.embedding.as_ref(),
                &policy,
                query,
            )
            .await?
        } else {
            None
        };
        let tx = self
            .db
            .begin_with_config(Some(IsolationLevel::RepeatableRead), None)
            .await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let (raw, source_time) =
            validate_materialization_source(&tx, &source, query, original, policy.version).await?;
        let mut lease = ContextLease {
            id: Uuid::new_v4(),
            purpose: ContextPurpose::Generation,
            materialized_digest: String::new(),
            source: source.clone(),
            policy_version: policy.version,
            dependencies: vec![ContextEvidence::UserMessage {
                id: source.source_event_id.unwrap(),
                session_id: source.conversation_id.unwrap(),
                source_privacy_epoch: source.source_privacy_epoch,
                content_digest: digest(&json!(raw))?,
            }],
            valid_until: None,
        };
        let mut history = Vec::new();
        if let Some(original) = original {
            merge(&mut lease.dependencies, &original.lease.dependencies)?;
            lease.valid_until = original.lease.valid_until;
            history = original.history.clone();
        }
        let mut remaining = policy.policy.max_memory_chars;
        let mut nodes = Vec::new();
        if reads_memory {
            let candidates = crate::providers::memory::simple::retrieve_context_nodes(
                &tx,
                source.owner_id,
                query,
                retrieval.clone(),
                vector.as_ref(),
                &policy.policy,
            )
            .await?;
            for mut node in candidates {
                if node.sensitive
                    || node.visibility == "private_runtime"
                    || (policy.policy.sensitive_filter_enabled
                        && crate::memory_policy::detect_sensitive_memory(&node.content).is_some())
                {
                    continue;
                }
                if remaining == 0 {
                    break;
                }
                let Some(id) = node.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) else {
                    return Err(ContextError::Unavailable(
                        "canonical memory lacks identity".into(),
                    )
                    .into());
                };
                node.content = node.content.chars().take(remaining.min(240)).collect();
                let mut candidate_nodes = nodes.clone();
                candidate_nodes.push(node.clone());
                let rendered =
                    render_memory(&memory_context_pack_from_nodes(candidate_nodes, vec![]));
                let overflow = rendered
                    .chars()
                    .count()
                    .saturating_sub(policy.policy.max_memory_chars);
                if overflow > 0 {
                    node.content = node
                        .content
                        .chars()
                        .take(node.content.chars().count().saturating_sub(overflow))
                        .collect();
                }
                if node.content.trim().is_empty() {
                    continue;
                }
                let mut candidate = lease.clone();
                let expanded = async {
                    let (roots, until) = super::memory_appraisal::memory_source_roots(
                        &tx,
                        source.owner_id,
                        id,
                        node.revision,
                    )
                    .await?;
                    merge(&mut candidate.dependencies, &roots)?;
                    candidate.valid_until = earliest(candidate.valid_until, until);
                    validate_evidence(&tx, &candidate).await
                }
                .await;
                match expanded {
                    Ok(()) => lease = candidate,
                    Err(error) if super::appraisal::is_revoked(&error) => continue,
                    Err(error) => return Err(error),
                }
                nodes.push(node);
                remaining = policy.policy.max_memory_chars.saturating_sub(
                    render_memory(&memory_context_pack_from_nodes(nodes.clone(), vec![]))
                        .chars()
                        .count(),
                );
            }
        }
        let mut graph = Vec::new();
        if reads_memory && remaining > 0 {
            let roots = nodes
                .iter()
                .filter_map(|node| node.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()))
                .collect::<Vec<_>>();
            let edges =
                graph_candidates(&tx, source.owner_id, &roots, policy.policy.graph_depth).await?;
            for edge in edges {
                let projected = match graph_projection(&tx, source.owner_id, edge).await {
                    Ok(value) => value,
                    Err(error) if super::appraisal::is_revoked(&error) => continue,
                    Err(error) => return Err(error),
                };
                let Some((projection, roots, expires)) = projected else {
                    continue;
                };
                if let Some(layers) = &retrieval.layers {
                    let ids = roots
                        .iter()
                        .filter_map(|root| match root {
                            ContextEvidence::Memory { id, .. } => Some(*id),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    let allowed=tx.query_one(stmt("SELECT COUNT(*) AS n FROM memory_items WHERE id=ANY(ARRAY(SELECT jsonb_array_elements_text($1)::uuid)) AND memory_layer=ANY($2::text[])",vec![json!(ids).into(),layers.clone().into()])).await?.unwrap().try_get::<i64>("","n")?;
                    if allowed != ids.len() as i64 {
                        continue;
                    }
                }
                let source_label = projection["source"].as_str().unwrap_or_default();
                let target_label = projection["target"].as_str().unwrap_or_default();
                if policy.policy.sensitive_filter_enabled
                    && [source_label, target_label]
                        .iter()
                        .any(|v| crate::memory_policy::detect_sensitive_memory(v).is_some())
                {
                    continue;
                }
                let snippet = GraphSnippet {
                    source: source_label.into(),
                    relation: projection["relation"].as_str().unwrap_or_default().into(),
                    target: target_label.into(),
                    confidence: projection["confidence"].as_f64().unwrap_or(0.0) as f32,
                };
                let mut candidate_graph = graph.clone();
                candidate_graph.push(snippet.clone());
                let rendered = render_memory(&memory_context_pack_from_nodes(
                    nodes.clone(),
                    candidate_graph,
                ));
                if rendered.chars().count() > policy.policy.max_memory_chars {
                    continue;
                }
                let mut refs = roots;
                refs.push(ContextEvidence::Graph {
                    edge_id: edge,
                    projection_digest: digest(&projection)?,
                });
                // Validate the complete candidate against this original source
                // before adding its roots. A revoked optional graph result must
                // not poison an otherwise valid new generation.
                let mut candidate = lease.clone();
                let admitted = async {
                    merge(&mut candidate.dependencies, &refs)?;
                    candidate.valid_until = earliest(candidate.valid_until, expires);
                    validate_evidence(&tx, &candidate).await
                }
                .await;
                match admitted {
                    Ok(()) => lease = candidate,
                    Err(error) if super::appraisal::is_revoked(&error) => continue,
                    Err(error) => return Err(error),
                }
                graph.push(snippet);
            }
        }
        let memory = memory_context_pack_from_nodes(nodes, graph);
        let memory_prompt = render_memory(&memory);
        if original.is_none() {
            // A bounded SQL window; current input is represented separately and
            // future messages cannot slip into this request's model context.
            let rows=tx.query_all(stmt("SELECT m.id,m.role,m.content,m.source_privacy_epoch,r.response_id,r.context_id,r.final_content FROM chat_messages m LEFT JOIN delivery_responses r ON r.assistant_message_id=m.id AND r.user_id=$2 AND r.conversation_id=m.session_id AND r.status='committed' WHERE m.session_id=$1 AND (m.created_at,m.id)<($3,$4) AND ((m.role='user' AND m.user_id=$5 AND m.source_privacy_epoch=$6 AND m.source_received_at IS NOT NULL) OR (m.role='assistant' AND r.context_id IS NOT NULL)) ORDER BY m.created_at DESC,m.id DESC LIMIT $7",vec![source.conversation_id.into(),source.owner_id.into(),source_time.into(),source.source_event_id.into(),source.owner_id.to_string().into(),source.source_privacy_epoch.into(),(policy.policy.max_history_messages as i64).into()])).await?;
            let mut bytes = raw.len() + serde_json::to_vec(&memory)?.len();
            for row in rows {
                let id: Uuid = row.try_get("", "id")?;
                let role: String = row.try_get("", "role")?;
                let content: String = row.try_get("", "content")?;
                if bytes.saturating_add(content.len()) > MAX_CONTEXT_BYTES {
                    continue;
                }
                let mut candidate = lease.clone();
                let result: ContextResult<()> = async {
                    if role == "user" {
                        merge(
                            &mut candidate.dependencies,
                            &[ContextEvidence::UserMessage {
                                id,
                                session_id: source.conversation_id.unwrap(),
                                source_privacy_epoch: source.source_privacy_epoch,
                                content_digest: digest(&json!(content))?,
                            }],
                        )?;
                    } else {
                        let context: Uuid = row.try_get("", "context_id")?;
                        let response: Uuid = row.try_get("", "response_id")?;
                        if row
                            .try_get::<Value>("", "final_content")?
                            .get("text")
                            .and_then(Value::as_str)
                            != Some(content.as_str())
                        {
                            anyhow::bail!(
                                "assistant history does not match its committed response"
                            );
                        }
                        let parent = load(&tx, source.owner_id, context).await?;
                        if parent.policy_version != policy.version
                            || parent.source.source_privacy_epoch != source.source_privacy_epoch
                            || parent.purpose != ContextPurpose::Generation
                        {
                            return Err(ContextError::Revoked(
                                "historical context no longer current".into(),
                            )
                            .into());
                        }
                        ensure_live(&tx, parent.valid_until).await?;
                        merge(&mut candidate.dependencies, &parent.dependencies)?;
                        merge(
                            &mut candidate.dependencies,
                            &[ContextEvidence::DerivedMessage {
                                id,
                                response_id: response,
                                context_id: context,
                                content_digest: digest(&json!(content))?,
                            }],
                        )?;
                        candidate.valid_until = earliest(candidate.valid_until, parent.valid_until);
                    }
                    validate_evidence(&tx, &candidate).await?;
                    Ok(())
                }
                .await;
                match result {
                    Ok(()) => {
                        lease = candidate;
                        bytes += content.len();
                        history.push(ContextHistoryMessage {
                            id,
                            message: ChatMessage::new(&role, &content),
                        });
                    }
                    Err(error) if error.downcast_ref::<sea_orm::DbErr>().is_some() => {
                        return Err(error);
                    }
                    Err(_) => (), // Invalid or over-budget whole histories never contribute text.
                }
            }
            history.reverse();
        }
        let mut material = GenerationContext {
            policy: policy.policy,
            lease,
            source_text: raw,
            extensions: original.map_or_else(Vec::new, |old| old.extensions.clone()),
            memory,
            prior_memory: original.map_or_else(Vec::new, |old| {
                let mut prior = old.prior_memory.clone();
                prior.push(old.memory.clone());
                prior
            }),
            history,
            memory_prompt,
            affect: original.and_then(|context| context.affect.clone()),
        };
        if original.is_some() {
            material.extensions.push(json!({"memory_query":query,"max_results":retrieval.max_results,"min_similarity":retrieval.min_similarity,"layers":retrieval.layers}));
        }
        let payload = materialized_payload(&material);
        if serde_json::to_vec(&payload)?.len() > MAX_CONTEXT_BYTES {
            return Err(ContextError::BoundsExceeded.into());
        }
        validate_evidence(&tx, &material.lease).await?;
        let key = if original.is_some() {
            format!("memory:{}", material.lease.id)
        } else {
            "generation".into()
        };
        persist(&tx, &mut material.lease, &key, "generation", &payload).await?;
        let id = material.lease.id;
        tx.commit()
            .await
            .map_err(|_| ContextError::CommitUnknown { context_id: id })?;
        Ok(material)
    }

    pub async fn source_notice(&self, source: ContextSource) -> ContextResult<ContextLease> {
        self.source_notice_for(source, SourceNotice::ProcessingFailedZh)
            .await
    }
    pub async fn source_notice_for(
        &self,
        source: ContextSource,
        notice: SourceNotice,
    ) -> ContextResult<ContextLease> {
        let policy = self.policy_store().current().await?;
        let tx = self.db.begin().await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        lock_authority(&tx, &source, policy.version).await?;
        let mut lease = ContextLease {
            id: Uuid::new_v4(),
            purpose: ContextPurpose::SourceNotice,
            materialized_digest: String::new(),
            source,
            policy_version: policy.version,
            dependencies: Vec::new(),
            valid_until: None,
        };
        persist(
            &tx,
            &mut lease,
            "notice",
            "source_notice",
            &json!({"text":notice.text()}),
        )
        .await?;
        let id = lease.id;
        tx.commit()
            .await
            .map_err(|_| ContextError::CommitUnknown { context_id: id })?;
        Ok(lease)
    }

    pub async fn merge_cache(
        &self,
        original: &GenerationContext,
        identity: CacheIdentity,
    ) -> ContextResult<GenerationContext> {
        let tx = self
            .db
            .begin_with_config(Some(IsolationLevel::RepeatableRead), None)
            .await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        validate_binding(&tx, &original.lease.binding(), ContextUse::CacheReuse).await?;
        let cache = cache_projection(&tx, original.source().owner_id, identity.cache_id)
            .await?
            .ok_or_else(|| ContextError::Revoked("cache no longer available".into()))?;
        anyhow::ensure!(
            cache.revision == identity.revision
                && cache.context_id == identity.context_id
                && cache.scope == identity.scope,
            "cache identity changed"
        );
        let parent = validate_lease(
            &tx,
            original.source().owner_id,
            cache.context_id,
            ContextUse::CacheReuse,
        )
        .await?
        .lease;
        anyhow::ensure!(
            parent.policy_version == original.lease.policy_version
                && parent.source.source_privacy_epoch == original.source().source_privacy_epoch
                && parent.purpose == ContextPurpose::Generation,
            "cache source authority differs from current request"
        );
        let mut merged = original.clone();
        merged.lease.id = Uuid::new_v4();
        merge(&mut merged.lease.dependencies, &parent.dependencies)?;
        merge(
            &mut merged.lease.dependencies,
            &[ContextEvidence::Cache {
                id: identity.cache_id,
                revision: cache.revision,
                context_id: cache.context_id,
                scope: cache.scope,
                query_digest: cache.query_digest,
                content_digest: cache.content_digest,
            }],
        )?;
        merged.lease.valid_until = earliest(
            earliest(merged.lease.valid_until, parent.valid_until),
            Some(cache.expires_at),
        );
        merged.extensions.push(json!({"cache_id":identity.cache_id,"revision":cache.revision,"response_text":cache.text}));
        let payload = materialized_payload(&merged);
        if serde_json::to_vec(&payload)?.len() > MAX_CONTEXT_BYTES {
            return Err(ContextError::BoundsExceeded.into());
        }
        validate_evidence(&tx, &merged.lease).await?;
        let key = format!("cache:{}", merged.lease.id);
        persist(&tx, &mut merged.lease, &key, "generation", &payload).await?;
        let id = merged.lease.id;
        tx.commit()
            .await
            .map_err(|_| ContextError::CommitUnknown { context_id: id })?;
        Ok(merged)
    }
    pub async fn extend_execution(
        &self,
        original: &GenerationContext,
        effect_id: Uuid,
    ) -> ContextResult<(GenerationContext, Value)> {
        let tx = self
            .db
            .begin_with_config(Some(IsolationLevel::RepeatableRead), None)
            .await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        validate_binding(&tx, &original.lease.binding(), ContextUse::Execution).await?;
        let source = original.source();
        let row=tx.query_one(stmt("SELECT context_id,source_received_at,source_event_id,source_privacy_epoch FROM execution_effects WHERE effect_id=$1 AND user_id=$2 AND request_id=$3 AND context_id IS NOT NULL",vec![effect_id.into(),source.owner_id.into(),source.request_id.into()])).await?.ok_or_else(||ContextError::Revoked("execution is not a sourced effect for this request".into()))?;
        anyhow::ensure!(
            row.try_get::<Option<Uuid>>("", "source_event_id")? == source.source_event_id
                && row.try_get::<Option<i64>>("", "source_privacy_epoch")?
                    == Some(source.source_privacy_epoch)
                && row
                    .try_get::<DateTime<Utc>>("", "source_received_at")?
                    .timestamp_micros()
                    == source.source_received_at.timestamp_micros(),
            "execution original source differs"
        );
        let parent = validate_lease(
            &tx,
            source.owner_id,
            row.try_get("", "context_id")?,
            ContextUse::Execution,
        )
        .await?
        .lease;
        anyhow::ensure!(
            parent.purpose == ContextPurpose::Generation
                && parent.policy_version == original.lease.policy_version
                && same_source(&parent.source, source),
            "execution context is not a valid generation source"
        );
        let completion = execution_projection(&tx, source.owner_id, effect_id)
            .await?
            .ok_or_else(|| ContextError::Revoked("execution disappeared".into()))?;
        let mut merged = original.clone();
        merged.lease.id = Uuid::new_v4();
        merge(&mut merged.lease.dependencies, &parent.dependencies)?;
        merge(
            &mut merged.lease.dependencies,
            &[ContextEvidence::Execution {
                effect_id,
                intent_digest: completion["intent_digest"]
                    .as_str()
                    .ok_or_else(|| {
                        ContextError::Unavailable("execution intent digest missing".into())
                    })?
                    .into(),
                completion_digest: digest(&completion)?,
            }],
        )?;
        merged.lease.valid_until = earliest(merged.lease.valid_until, parent.valid_until);
        merged
            .extensions
            .push(json!({"effect_id":effect_id,"completion":completion}));
        let payload = materialized_payload(&merged);
        if serde_json::to_vec(&payload)?.len() > MAX_CONTEXT_BYTES {
            return Err(ContextError::BoundsExceeded.into());
        }
        validate_evidence(&tx, &merged.lease).await?;
        let key = format!("execution:{}", merged.lease.id);
        persist(&tx, &mut merged.lease, &key, "generation", &payload).await?;
        let id = merged.lease.id;
        tx.commit()
            .await
            .map_err(|_| ContextError::CommitUnknown { context_id: id })?;
        Ok((merged, completion))
    }
}
async fn validate_materialization_source(
    tx: &DatabaseTransaction,
    source: &ContextSource,
    query: &str,
    original: Option<&GenerationContext>,
    policy_version: i64,
) -> ContextResult<(String, DateTime<Utc>)> {
    lock_authority(tx, source, policy_version).await?;
    let source_row = tx
        .query_one(stmt(
            "SELECT content,created_at FROM chat_messages WHERE id=$1",
            vec![source.source_event_id.into()],
        ))
        .await?
        .ok_or_else(|| ContextError::Revoked("source message missing".into()))?;
    let raw: String = source_row.try_get("", "content")?;
    if let Some(original) = original {
        validate_binding(tx, &original.lease.binding(), ContextUse::OutputAdmission).await?;
    } else {
        anyhow::ensure!(
            raw == query,
            "initial generation must use the persisted raw user message"
        );
    }
    Ok((raw, source_row.try_get("", "created_at")?))
}
pub(super) fn materialized_payload(context: &GenerationContext) -> Value {
    json!({"source_text":context.source_text,"memory":context.memory,"prior_memory":context.prior_memory,"history":context.history.iter().map(|m|json!({"id":m.id,"role":m.message.role,"content":m.message.content})).collect::<Vec<_>>(),"memory_prompt":context.memory_prompt,"extensions":context.extensions})
}
fn render_memory(pack: &crate::providers::adapter::MemoryContextPack) -> String {
    let mut sections = Vec::new();
    for (title, snippets) in [
        ("Preferences", &pack.user_preferences),
        ("Projects", &pack.project_context),
        ("Episodes", &pack.relevant_episodes),
        ("Guidance", &pack.affective_guidance),
        ("Boundaries", &pack.boundaries),
    ] {
        if !snippets.is_empty() {
            sections.push(format!(
                "{title}:\n{}",
                snippets
                    .iter()
                    .map(|s| format!("- {}", s.content))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
    }
    if !pack.graph_context.is_empty() {
        sections.push(format!(
            "Graph:\n{}",
            pack.graph_context
                .iter()
                .map(|s| format!("- {} {} {}", s.source, s.relation, s.target))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    sections.join("\n\n")
}
