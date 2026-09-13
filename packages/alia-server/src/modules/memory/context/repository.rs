use chrono::{DateTime, Utc};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseTransaction, Statement, Value as SqlValue,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::*;

pub(crate) fn stmt(sql: &str, values: Vec<SqlValue>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}
pub(crate) fn digest(value: &Value) -> ContextResult<String> {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let sorted: std::collections::BTreeMap<_, _> =
                    map.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
                Value::Object(sorted.into_iter().collect())
            }
            Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
            other => other.clone(),
        }
    }
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical(value))?)
    ))
}
fn revoked(reason: &str) -> anyhow::Error {
    ContextError::Revoked(reason.into()).into()
}

pub(crate) async fn lock_authority(
    tx: &DatabaseTransaction,
    source: &ContextSource,
    expected_policy: i64,
) -> ContextResult<PolicySnapshot> {
    anyhow::ensure!(
        !source.owner_id.is_nil()
            && !source.request_id.is_nil()
            && !source.channel_id.is_empty()
            && source.channel_id.len() <= 256
            && source.source_privacy_epoch >= 0,
        "invalid context source"
    );
    let policy = policy::read(tx, "FOR SHARE").await?;
    let now = tx
        .query_one(stmt("SELECT clock_timestamp() AS now", vec![]))
        .await?
        .unwrap()
        .try_get::<DateTime<Utc>>("", "now")?;
    if policy.version != expected_policy || policy.policy.paused_until.is_some_and(|t| t <= now) {
        return Err(ContextError::PolicyChanged.into());
    }
    let active = tx
        .query_one(stmt(
            "SELECT id FROM users WHERE id=$1 AND status='active' FOR SHARE",
            vec![source.owner_id.into()],
        ))
        .await?
        .is_some();
    if !active {
        return Err(revoked("owner is not active"));
    }
    let row = tx
        .query_one(stmt(
            "SELECT privacy_epoch FROM memory_privacy_state WHERE user_id=$1 FOR UPDATE",
            vec![source.owner_id.into()],
        ))
        .await?
        .ok_or_else(|| revoked("source privacy state is missing"))?;
    if row.try_get::<i64>("", "privacy_epoch")? != source.source_privacy_epoch {
        return Err(revoked("original privacy epoch changed"));
    }
    if let Some(session) = source.conversation_id {
        if tx.query_one(stmt("SELECT id FROM chat_sessions WHERE id=$1 AND creator_id=$2 AND channel_id=$3 FOR SHARE",vec![session.into(),source.owner_id.to_string().into(),source.channel_id.clone().into()])).await?.is_none() {
            return Err(revoked("source conversation is unavailable for owner/channel"));
        }
    }
    if let Some(id) = source.source_event_id {
        let row = tx.query_one(stmt("SELECT session_id,source_received_at,source_privacy_epoch FROM chat_messages WHERE id=$1 AND user_id=$2 AND role='user' FOR SHARE",vec![id.into(),source.owner_id.to_string().into()])).await?.ok_or_else(||revoked("original user source is missing"))?;
        if Some(row.try_get::<Uuid>("", "session_id")?) != source.conversation_id
            || row.try_get::<Option<i64>>("", "source_privacy_epoch")?
                != Some(source.source_privacy_epoch)
            || row
                .try_get::<Option<DateTime<Utc>>>("", "source_received_at")?
                .map(|v| v.timestamp_micros())
                != Some(source.source_received_at.timestamp_micros())
        {
            return Err(revoked("source stamp differs from the original message"));
        }
    }
    Ok(policy)
}

pub(crate) async fn load(
    tx: &DatabaseTransaction,
    owner: Uuid,
    id: Uuid,
) -> ContextResult<ContextLease> {
    let row = tx
        .query_one(stmt(
            "SELECT * FROM context_leases WHERE context_id=$1 AND owner_id=$2",
            vec![id.into(), owner.into()],
        ))
        .await?
        .ok_or_else(|| revoked("lease is missing or belongs to another owner"))?;
    let value: Value = row.try_get("", "dependencies")?;
    if digest(&value)? != row.try_get::<String>("", "dependencies_digest")? {
        return Err(revoked("lease evidence digest mismatch"));
    }
    let dependencies: Vec<ContextEvidence> = serde_json::from_value(value)?;
    check_bounds(&dependencies)?;
    Ok(ContextLease {
        id,
        purpose: serde_json::from_value(json!(row.try_get::<String>("", "purpose")?))?,
        materialized_digest: row.try_get("", "materialized_digest")?,
        source: ContextSource {
            owner_id: owner,
            request_id: row.try_get("", "request_id")?,
            channel_id: row.try_get("", "channel_id")?,
            conversation_id: row.try_get("", "conversation_id")?,
            source_event_id: row.try_get("", "source_event_id")?,
            source_received_at: row.try_get("", "source_received_at")?,
            source_privacy_epoch: row.try_get("", "source_privacy_epoch")?,
        },
        policy_version: row.try_get("", "policy_version")?,
        dependencies,
        valid_until: row.try_get("", "valid_until")?,
    })
}
pub(crate) async fn validate_binding(
    tx: &DatabaseTransaction,
    binding: &ContextBinding,
    purpose: ContextUse,
) -> ContextResult<ValidatedContext> {
    let validated = validate_lease(tx, binding.owner_id, binding.context_id, purpose).await?;
    let lease = validated.lease;
    if lease.source.request_id != binding.request_id
        || lease.source.source_event_id != binding.source_event_id
        || lease.source.source_privacy_epoch != binding.source_privacy_epoch
    {
        return Err(revoked(
            "context does not belong to this original request/source",
        ));
    }
    Ok(ValidatedContext {
        lease,
        policy: validated.policy,
    })
}
pub(crate) async fn validate_lease(
    tx: &DatabaseTransaction,
    owner: Uuid,
    id: Uuid,
    purpose: ContextUse,
) -> ContextResult<ValidatedContext> {
    let lease = load(tx, owner, id).await?;
    let policy = lock_authority(tx, &lease.source, lease.policy_version).await?;
    if !lease.authorizes(purpose) {
        return Err(revoked("context purpose does not authorize this operation"));
    }
    if lease.purpose == ContextPurpose::SourceNotice && lease.notice_text().is_none() {
        return Err(revoked("source notice has no trusted fixed literal"));
    }
    if !policy.policy.allows(purpose) {
        return Err(revoked("current policy disallows this derived use"));
    }
    ensure_live(tx, lease.valid_until).await?;
    validate_evidence(tx, &lease).await?;
    Ok(ValidatedContext {
        lease,
        policy: policy.policy,
    })
}
pub(crate) async fn ensure_live(
    tx: &DatabaseTransaction,
    expiry: Option<DateTime<Utc>>,
) -> ContextResult<()> {
    let live = tx
        .query_one(stmt(
            "SELECT ($1::timestamptz IS NULL OR $1>clock_timestamp()) AS live",
            vec![expiry.into()],
        ))
        .await?
        .unwrap()
        .try_get::<bool>("", "live")?;
    if !live {
        return Err(revoked("context dependency expired"));
    }
    Ok(())
}
pub(crate) fn check_bounds(refs: &[ContextEvidence]) -> ContextResult<()> {
    if refs.len() > MAX_EVIDENCE_REFS || serde_json::to_vec(refs)?.len() > MAX_EVIDENCE_BYTES {
        return Err(ContextError::BoundsExceeded.into());
    }
    Ok(())
}
pub(crate) fn merge(
    target: &mut Vec<ContextEvidence>,
    incoming: &[ContextEvidence],
) -> ContextResult<()> {
    let mut next = target.clone();
    for item in incoming {
        // Conflicting versions of one root cannot be hidden by deduplication.
        for old in &next {
            if evidence_key(old) == evidence_key(item) && old != item {
                return Err(revoked("conflicting versions of the same context evidence"));
            }
        }
        if !next.contains(item) {
            next.push(item.clone());
        }
    }
    next.sort_by_key(evidence_key);
    check_bounds(&next)?;
    *target = next;
    Ok(())
}
fn evidence_key(value: &ContextEvidence) -> (u8, Uuid) {
    match value {
        ContextEvidence::UserMessage { id, .. } => (0, *id),
        ContextEvidence::DerivedMessage { id, .. } => (1, *id),
        ContextEvidence::Memory { id, .. } => (2, *id),
        ContextEvidence::Graph { edge_id, .. } => (3, *edge_id),
        ContextEvidence::Cache { id, .. } => (4, *id),
        ContextEvidence::Execution { effect_id, .. } => (5, *effect_id),
    }
}

pub(crate) async fn validate_evidence(
    tx: &DatabaseTransaction,
    lease: &ContextLease,
) -> ContextResult<()> {
    let mut refs = lease.dependencies.clone();
    refs.sort_by_key(evidence_key);
    for reference in &refs {
        let owner = lease.source.owner_id;
        match reference {
            ContextEvidence::Memory{id,revision} => {
                if tx.query_one(stmt("SELECT id FROM memory_items WHERE id=$1 AND user_id=$2 AND tenant_id=$3 AND revision=$4 AND status='active' AND NOT sensitive AND visibility<>'private_runtime' AND (expires_at IS NULL OR expires_at>clock_timestamp()) FOR SHARE",vec![(*id).into(),owner.into(),Uuid::nil().into(),(*revision).into()])).await?.is_none() { return Err(revoked("memory revision is unavailable")); }
            },
            ContextEvidence::UserMessage{id,session_id,source_privacy_epoch,content_digest} => {
                let row=tx.query_one(stmt("SELECT m.content FROM chat_messages m JOIN chat_sessions s ON s.id=m.session_id WHERE m.id=$1 AND m.session_id=$2 AND s.creator_id=$3 AND m.user_id=$3 AND m.role='user' AND m.source_privacy_epoch=$4 AND m.source_received_at IS NOT NULL FOR SHARE OF m",vec![(*id).into(),(*session_id).into(),owner.to_string().into(),(*source_privacy_epoch).into()])).await?.ok_or_else(||revoked("user history source is missing"))?;
                if *source_privacy_epoch!=lease.source.source_privacy_epoch || digest(&json!(row.try_get::<String>("", "content")?))? != *content_digest { return Err(revoked("user history source changed")); }
            },
            ContextEvidence::DerivedMessage{id,response_id,context_id,content_digest} => {
                let row=tx.query_one(stmt("SELECT m.content,r.final_content FROM chat_messages m JOIN chat_sessions s ON s.id=m.session_id JOIN delivery_responses r ON r.assistant_message_id=m.id WHERE m.id=$1 AND r.response_id=$2 AND r.context_id=$3 AND r.user_id=$4 AND s.creator_id=$5 AND r.conversation_id=m.session_id AND r.status='committed' AND m.role='assistant' FOR SHARE OF m",vec![(*id).into(),(*response_id).into(),(*context_id).into(),owner.into(),owner.to_string().into()])).await?.ok_or_else(||revoked("derived history source is unavailable"))?;
                let content:String=row.try_get("", "content")?;
                if digest(&json!(content))? != *content_digest || row.try_get::<Value>("", "final_content")?.get("text").and_then(Value::as_str)!=Some(content.as_str()) { return Err(revoked("derived history content changed")); }
                validate_parent_header(tx,lease,*context_id).await?;
            },
            ContextEvidence::Graph{edge_id,projection_digest} => {
                let (projection,_,_)=graph_projection(tx,owner,*edge_id).await?.ok_or_else(||revoked("graph evidence disappeared"))?;
                if digest(&projection)?!=*projection_digest { return Err(revoked("graph projection changed")); }
            },
            ContextEvidence::Cache{id,revision,context_id,scope,query_digest,content_digest} => {
                let cache=cache_projection(tx,owner,*id).await?.ok_or_else(||revoked("cache evidence disappeared"))?;
                if cache.revision!=*revision || cache.context_id!=*context_id || cache.scope!=*scope || cache.query_digest!=*query_digest || cache.content_digest!=*content_digest { return Err(revoked("cache version changed")); }
                validate_parent_header(tx,lease,*context_id).await?;
            },
            ContextEvidence::Execution{effect_id,intent_digest,completion_digest} => {
                let projection=execution_projection(tx,owner,*effect_id).await?.ok_or_else(||revoked("execution result is unavailable"))?;
                if projection["intent_digest"].as_str()!=Some(intent_digest.as_str()) || digest(&projection)?!=*completion_digest { return Err(revoked("execution evidence changed")); }
                let parent=projection["context_id"].as_str().and_then(|value|Uuid::parse_str(value).ok()).ok_or_else(||revoked("execution source context missing"))?;
                validate_parent_header(tx,lease,parent).await?;
            },
        }
    }
    Ok(())
}
async fn validate_parent_header(
    tx: &DatabaseTransaction,
    current: &ContextLease,
    id: Uuid,
) -> ContextResult<()> {
    let parent = load(tx, current.source.owner_id, id).await?;
    if parent.policy_version != current.policy_version
        || parent.source.source_privacy_epoch != current.source.source_privacy_epoch
        || parent.purpose != ContextPurpose::Generation
    {
        return Err(revoked("derived context policy or epoch changed"));
    }
    lock_authority(tx, &parent.source, parent.policy_version).await?;
    ensure_live(tx, parent.valid_until).await?;
    // Flattening is checked again, so a stored reference cannot lose its roots.
    if !parent
        .dependencies
        .iter()
        .all(|reference| current.dependencies.contains(reference))
    {
        return Err(revoked("derived context is missing canonical roots"));
    }
    Ok(())
}
pub(crate) async fn persist(
    tx: &DatabaseTransaction,
    lease: &mut ContextLease,
    key: &str,
    purpose: &str,
    materialized: &Value,
) -> ContextResult<()> {
    check_bounds(&lease.dependencies)?;
    let evidence = json!(lease.dependencies);
    let materialized_digest = digest(materialized)?;
    lease.materialized_digest = materialized_digest.clone();
    let s = &lease.source;
    let inserted=tx.execute(stmt("INSERT INTO context_leases(context_id,owner_id,request_id,context_key,purpose,channel_id,conversation_id,source_event_id,source_received_at,source_privacy_epoch,policy_version,dependencies,dependencies_digest,materialized_digest,valid_until) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) ON CONFLICT(owner_id,request_id,context_key) DO NOTHING",vec![lease.id.into(),s.owner_id.into(),s.request_id.into(),key.into(),purpose.into(),s.channel_id.clone().into(),s.conversation_id.into(),s.source_event_id.into(),s.source_received_at.into(),s.source_privacy_epoch.into(),lease.policy_version.into(),evidence.clone().into(),digest(&evidence)?.into(),materialized_digest.clone().into(),lease.valid_until.into()])).await?.rows_affected();
    if inserted == 0 {
        let row=tx.query_one(stmt("SELECT context_id,materialized_digest FROM context_leases WHERE owner_id=$1 AND request_id=$2 AND context_key=$3",vec![s.owner_id.into(),s.request_id.into(),key.into()])).await?.unwrap();
        let old = load(tx, s.owner_id, row.try_get("", "context_id")?).await?;
        if !same_source(&old.source, &lease.source)
            || old.policy_version != lease.policy_version
            || old.dependencies != lease.dependencies
            || old.purpose != lease.purpose
            || old.valid_until != lease.valid_until
            || row.try_get::<String>("", "materialized_digest")? != materialized_digest
        {
            return Err(revoked(
                "request context is already sealed to different material",
            ));
        }
        lease.id = old.id;
    }
    Ok(())
}

pub(crate) fn same_source(left: &ContextSource, right: &ContextSource) -> bool {
    left.owner_id == right.owner_id
        && left.request_id == right.request_id
        && left.channel_id == right.channel_id
        && left.conversation_id == right.conversation_id
        && left.source_event_id == right.source_event_id
        && left.source_privacy_epoch == right.source_privacy_epoch
        && left.source_received_at.timestamp_micros() == right.source_received_at.timestamp_micros()
}

pub(crate) async fn execution_projection(
    tx: &DatabaseTransaction,
    owner: Uuid,
    effect: Uuid,
) -> ContextResult<Option<Value>> {
    let row=tx.query_one(stmt("SELECT model_call_id,tool_name,status,intent_digest,completion,context_id FROM execution_effects WHERE effect_id=$1 AND user_id=$2 AND context_id IS NOT NULL FOR SHARE",vec![effect.into(),owner.into()])).await?;
    row.map(|r|Ok(json!({"effect_id":effect,"model_call_id":r.try_get::<String>("", "model_call_id")?,"tool_name":r.try_get::<String>("", "tool_name")?,"status":r.try_get::<String>("", "status")?,"intent_digest":r.try_get::<String>("", "intent_digest")?,"completion":r.try_get::<Option<Value>>("", "completion")?,"context_id":r.try_get::<Uuid>("", "context_id")?}))).transpose()
}

pub(crate) async fn graph_projection(
    tx: &DatabaseTransaction,
    owner: Uuid,
    edge: Uuid,
) -> ContextResult<Option<(Value, Vec<ContextEvidence>, Option<DateTime<Utc>>)>> {
    let row=tx.query_one(stmt("SELECT e.id,e.evidence_memory_id,e.source_revision,e.valid_to,src.id AS src_id,src.label AS source,src.properties AS src_properties,dst.id AS dst_id,dst.label AS target,dst.properties AS dst_properties,e.relation,e.confidence,m.expires_at FROM memory_live_edges e JOIN memory_nodes src ON src.id=e.src_node_id AND src.user_id=e.user_id AND src.tenant_id=e.tenant_id JOIN memory_nodes dst ON dst.id=e.dst_node_id AND dst.user_id=e.user_id AND dst.tenant_id=e.tenant_id JOIN memory_items m ON m.id=e.evidence_memory_id WHERE e.id=$1 AND e.user_id=$2 AND e.tenant_id=$3",vec![edge.into(),owner.into(),Uuid::nil().into()])).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut roots = vec![ContextEvidence::Memory {
        id: row.try_get("", "evidence_memory_id")?,
        revision: row.try_get("", "source_revision")?,
    }];
    let mut expiry = earliest(row.try_get("", "valid_to")?, row.try_get("", "expires_at")?);
    let src: Value = row.try_get("", "src_properties")?;
    let dst: Value = row.try_get("", "dst_properties")?;
    for properties in [&src, &dst] {
        if let Some(id) = properties.get("memory_id").and_then(Value::as_str) {
            let id = Uuid::parse_str(id)?;
            let memory=tx.query_one(stmt("SELECT revision,expires_at FROM memory_items WHERE id=$1 AND user_id=$2 AND tenant_id=$3 AND status='active' AND NOT sensitive AND visibility<>'private_runtime' AND (expires_at IS NULL OR expires_at>clock_timestamp())",vec![id.into(),owner.into(),Uuid::nil().into()])).await?.ok_or_else(||revoked("graph endpoint memory is not live"))?;
            merge(
                &mut roots,
                &[ContextEvidence::Memory {
                    id,
                    revision: memory.try_get("", "revision")?,
                }],
            )?;
            expiry = earliest(expiry, memory.try_get("", "expires_at")?);
        }
    }
    let projection = json!({"edge_id":edge,"source_id":row.try_get::<Uuid>("", "src_id")?,"target_id":row.try_get::<Uuid>("", "dst_id")?,"source":row.try_get::<String>("", "source")?,"target":row.try_get::<String>("", "target")?,"relation":row.try_get::<String>("", "relation")?,"confidence":row.try_get::<f32>("", "confidence")?,"source_memory":src.get("memory_id"),"target_memory":dst.get("memory_id"),"roots":roots,"valid_to":row.try_get::<Option<DateTime<Utc>>>("", "valid_to")?});
    Ok(Some((projection, roots, expiry)))
}
/// Bound each expansion and the whole walk. A small root set must not launch
/// an unbounded recursive SQL search merely because the graph is dense.
pub(crate) async fn graph_candidates(
    tx: &DatabaseTransaction,
    owner: Uuid,
    roots: &[Uuid],
    depth: u32,
) -> ContextResult<Vec<Uuid>> {
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    let mut selected = Vec::new();
    let mut frontier = Vec::<Uuid>::new();
    for hop in 0..depth.clamp(1, 4) {
        let rows=tx.query_all(stmt("SELECT id,src_node_id,dst_node_id FROM memory_live_edges WHERE user_id=$1 AND tenant_id=$2 AND NOT(id=ANY(ARRAY(SELECT jsonb_array_elements_text($4)::uuid))) AND (($6 AND evidence_memory_id=ANY(ARRAY(SELECT jsonb_array_elements_text($3)::uuid))) OR (NOT $6 AND (src_node_id=ANY(ARRAY(SELECT jsonb_array_elements_text($3)::uuid)) OR dst_node_id=ANY(ARRAY(SELECT jsonb_array_elements_text($3)::uuid))))) ORDER BY confidence DESC,created_at DESC,id LIMIT $5",vec![owner.into(),Uuid::nil().into(),json!(if hop==0{roots}else{frontier.as_slice()}).into(),json!(selected).into(),((MAX_GRAPH_RESULTS-selected.len()) as i64).into(),(hop==0).into()])).await?;
        if rows.is_empty() {
            break;
        }
        frontier.clear();
        for row in rows {
            selected.push(row.try_get::<Uuid>("", "id")?);
            frontier.push(row.try_get("", "src_node_id")?);
            frontier.push(row.try_get("", "dst_node_id")?);
        }
        frontier.sort_unstable();
        frontier.dedup();
        if selected.len() >= MAX_GRAPH_RESULTS {
            break;
        }
    }
    Ok(selected)
}
pub(crate) fn earliest(
    a: Option<DateTime<Utc>>,
    b: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

pub(crate) struct CacheProjection {
    pub revision: i64,
    pub context_id: Uuid,
    pub scope: String,
    pub query_digest: String,
    pub content_digest: String,
    pub expires_at: DateTime<Utc>,
    pub text: String,
}
pub(crate) async fn cache_projection(
    tx: &DatabaseTransaction,
    owner: Uuid,
    id: Uuid,
) -> ContextResult<Option<CacheProjection>> {
    let row=tx.query_one(stmt("SELECT revision,context_id,scope,query_hash,query_text,response_text,content_digest,expires_at FROM llm_cache WHERE id=$1 AND user_id=$2 AND context_id IS NOT NULL AND expires_at>clock_timestamp() AND status='active' FOR SHARE",vec![id.into(),owner.into()])).await?;
    row.map(|r| {
        let text: String = r.try_get("", "response_text")?;
        let content_digest = digest(&json!({"response_text":text}))?;
        anyhow::ensure!(
            content_digest == r.try_get::<String>("", "content_digest")?,
            "cache content digest mismatch"
        );
        let query: String = r.try_get("", "query_text")?;
        let query_digest = format!("{:x}", Sha256::digest(query.as_bytes()));
        anyhow::ensure!(
            query_digest == r.try_get::<String>("", "query_hash")?,
            "cache query digest mismatch"
        );
        Ok(CacheProjection {
            revision: r.try_get("", "revision")?,
            context_id: r.try_get("", "context_id")?,
            scope: r.try_get("", "scope")?,
            query_digest,
            content_digest,
            expires_at: r.try_get("", "expires_at")?,
            text,
        })
    })
    .transpose()
}
