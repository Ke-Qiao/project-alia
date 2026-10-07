//! Canonical appraisal inputs and transaction gates. Interpreter drafts carry
//! no authority; this module constructs identities from durable source rows.
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction, QueryResult, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{repository::*, *};
macro_rules! ensure_current {
    ($condition:expr,$message:expr) => {
        if !$condition {
            return Err(ContextError::Revoked($message.into()).into());
        }
    };
}
use crate::expression::{
    AffectSnapshot, AppraisalEvent, AppraisalScope, AppraisalSource, RelationshipProfile,
};
use crate::modules::memory::MemoryRecallEvent;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppraisalFact {
    UserMessage {
        id: Uuid,
        content_digest: String,
    },
    Recall {
        context_id: Uuid,
        material_digest: String,
    },
    ResponseCommitted {
        response_id: Uuid,
        content_digest: String,
    },
    Execution {
        effect_id: Uuid,
        projection_digest: String,
    },
}

#[derive(Debug, Clone)]
pub struct AppraisalInput {
    pub(crate) rebuild: Option<super::rebuild::RebuildClaim>,
    pub(crate) lease: ContextLease,
    pub(crate) relationship_epoch: i64,
    pub(crate) policy: AppraisalPolicy,
    pub(crate) fact: AppraisalFact,
    pub(crate) occurrence: String,
    pub(crate) input_digest: String,
    pub(crate) text: String,
    pub(crate) recalls: Vec<MemoryRecallEvent>,
    pub(crate) effective_at: DateTime<Utc>,
}
impl AppraisalInput {
    pub fn id(&self) -> Uuid {
        stable_id(
            &serde_json::to_vec(&json!([
                self.lease.source.owner_id,
                self.lease.source.request_id,
                self.occurrence,
                self.policy.interpreter_version,
                self.lease.policy_version
            ]))
            .expect("appraisal identity is serializable"),
        )
    }
    pub fn binding(&self) -> ContextBinding {
        self.lease.binding()
    }
    pub fn policy(&self) -> &AppraisalPolicy {
        &self.policy
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn recalls(&self) -> &[MemoryRecallEvent] {
        &self.recalls
    }
    pub fn effective_at(&self) -> DateTime<Utc> {
        self.effective_at
    }
    pub fn fact(&self) -> &AppraisalFact {
        &self.fact
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppraisalReceipt {
    pub appraisal_id: Uuid,
    pub revision: i64,
    pub result_digest: String,
    pub event_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffectProjection {
    pub(crate) id: Uuid,
    pub(crate) owner: Uuid,
    pub(crate) relationship_epoch: i64,
    pub(crate) policy_version: i64,
    pub(crate) reducer_version: String,
    pub(crate) as_of: DateTime<Utc>,
    pub(crate) snapshot: AffectSnapshot,
    pub(crate) profile: RelationshipProfile,
    pub(crate) appraisals: Vec<AppraisalReceipt>,
}
impl AffectProjection {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn snapshot(&self) -> &AffectSnapshot {
        &self.snapshot
    }
    pub fn profile(&self) -> &RelationshipProfile {
        &self.profile
    }
    pub fn as_of(&self) -> DateTime<Utc> {
        self.as_of
    }
    pub fn appraisals(&self) -> &[AppraisalReceipt] {
        &self.appraisals
    }
}

pub(crate) async fn relationship_epoch(
    tx: &DatabaseTransaction,
    owner: Uuid,
) -> ContextResult<i64> {
    crate::modules::memory::source::lock_relationship_epoch(tx, owner).await
}

async fn original_relationship_epoch(
    tx: &DatabaseTransaction,
    lease: &ContextLease,
) -> ContextResult<i64> {
    let current = relationship_epoch(tx, lease.source.owner_id).await?;
    let id = lease
        .source
        .source_event_id
        .ok_or_else(|| ContextError::Revoked("appraisal has no original source".into()))?;
    let original = tx.query_one(stmt("SELECT source_relationship_epoch FROM chat_messages WHERE id=$1 AND user_id=$2 AND role='user' FOR SHARE",vec![id.into(),lease.source.owner_id.to_string().into()])).await?.ok_or_else(||ContextError::Revoked("appraisal source unavailable".into()))?.try_get::<Option<i64>>("", "source_relationship_epoch")?;
    if original != Some(current) {
        return Err(ContextError::Revoked(
            "original relationship epoch changed or was never recorded".into(),
        )
        .into());
    }
    Ok(current)
}

impl ContextService {
    /// Recover an unknown acknowledgement using the original prepared identity.
    /// This is a current-authority query, not permission to revive old work.
    pub async fn appraisal_receipt(
        &self,
        input: &AppraisalInput,
    ) -> ContextResult<Option<AppraisalReceipt>> {
        let tx = self.db.begin().await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let current =
            Self::validate_in_transaction(&tx, &input.binding(), ContextUse::AppraisalWrite)
                .await?;
        ensure_current!(
            original_relationship_epoch(&tx, &current.lease).await? == input.relationship_epoch,
            "appraisal relationship authority changed"
        );
        let Some(row) = tx
            .query_one(stmt(
                "SELECT * FROM appraisal_records WHERE appraisal_id=$1 AND owner_id=$2 FOR SHARE",
                vec![input.id().into(), input.binding().owner_id.into()],
            ))
            .await?
        else {
            tx.commit().await?;
            return Ok(None);
        };
        ensure_current!(
            row.try_get::<String>("", "input_digest")? == input.input_digest
                && row.try_get::<Uuid>("", "context_id")? == input.lease.id
                && row.try_get::<String>("", "status")? == "active",
            "appraisal receipt is not the original active intent"
        );
        ensure_live(&tx, Some(row.try_get("", "valid_until")?)).await?;
        ensure_current!(
            record_roots(&row)? == current.lease.dependencies,
            "appraisal receipt changed its roots"
        );
        validate_fact(&tx, &current.lease, &input.fact).await?;
        let events = record_events(&row)?;
        let receipt = AppraisalReceipt {
            appraisal_id: input.id(),
            revision: row.try_get("", "revision")?,
            result_digest: row.try_get("", "result_digest")?,
            event_count: events.len(),
        };
        tx.commit().await?;
        Ok(Some(receipt))
    }

    pub async fn prepare_user_appraisal(
        &self,
        original: &GenerationContext,
    ) -> ContextResult<AppraisalInput> {
        self.prepare_appraisal(original, None).await
    }
    pub async fn prepare_recall_appraisal(
        &self,
        original: &GenerationContext,
    ) -> ContextResult<AppraisalInput> {
        self.prepare_appraisal(original, Some(())).await
    }
    async fn prepare_appraisal(
        &self,
        original: &GenerationContext,
        recall: Option<()>,
    ) -> ContextResult<AppraisalInput> {
        let tx = self.db.begin().await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let validated = Self::validate_in_transaction(
            &tx,
            &original.lease.binding(),
            ContextUse::AppraisalWrite,
        )
        .await?;
        let epoch = original_relationship_epoch(&tx, &validated.lease).await?;
        anyhow::ensure!(
            validated.lease.materialized_digest == original.lease.materialized_digest,
            "materialized appraisal input changed"
        );
        let source = &validated.lease.source;
        let (fact, occurrence, text, recalls, material) = if recall.is_some() {
            let recalls = original.all_materialized_memory().recall_events;
            let material = serde_json::to_value(&recalls)?;
            let content_digest = digest(&material)?;
            (
                AppraisalFact::Recall {
                    context_id: original.lease.id,
                    material_digest: original.lease.materialized_digest.clone(),
                },
                format!("recall:{content_digest}"),
                String::new(),
                recalls,
                material,
            )
        } else {
            let id = source.source_event_id.unwrap();
            let row = tx
                .query_one(stmt(
                    "SELECT content FROM chat_messages WHERE id=$1 FOR SHARE",
                    vec![id.into()],
                ))
                .await?
                .ok_or_else(|| ContextError::Revoked("user source missing".into()))?;
            let text: String = row.try_get("", "content")?;
            let material = json!(text);
            (
                AppraisalFact::UserMessage {
                    id,
                    content_digest: digest(&material)?,
                },
                format!("user:{id}"),
                text,
                Vec::new(),
                material,
            )
        };
        anyhow::ensure!(
            serde_json::to_vec(&material)?.len() <= MAX_CONTEXT_BYTES,
            "appraisal input exceeds bounds"
        );
        let input_digest = digest(
            &json!({"source":source,"relationship_epoch":epoch,"policy":validated.policy.appraisal.interpretation_digest()?,"fact":fact,"material":material,"roots":validated.lease.dependencies}),
        )?;
        let input = AppraisalInput {
            rebuild: None,
            effective_at: source.source_received_at,
            lease: validated.lease,
            relationship_epoch: epoch,
            policy: validated.policy.appraisal,
            fact,
            occurrence,
            input_digest,
            text,
            recalls,
        };
        tx.commit().await?;
        Ok(input)
    }

    pub async fn prepare_committed_response_appraisal(
        &self,
        binding: &ContextBinding,
        response_id: Uuid,
    ) -> ContextResult<AppraisalInput> {
        let tx = self.db.begin().await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let validated =
            Self::validate_in_transaction(&tx, binding, ContextUse::AppraisalWrite).await?;
        let epoch = original_relationship_epoch(&tx, &validated.lease).await?;
        let row = tx.query_one(stmt("SELECT final_content,updated_at FROM delivery_responses WHERE response_id=$1 AND context_id=$2 AND user_id=$3 AND request_id=$4 AND status='committed' FOR SHARE",vec![response_id.into(),binding.context_id.into(),binding.owner_id.into(),binding.request_id.into()])).await?.ok_or_else(||ContextError::Revoked("response is not canonically committed".into()))?;
        let content: Value = row.try_get("", "final_content")?;
        let text = content
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| ContextError::Unavailable("committed response has no text".into()))?
            .to_owned();
        let fact = AppraisalFact::ResponseCommitted {
            response_id,
            content_digest: digest(&content)?,
        };
        let input_digest = digest(
            &json!({"source":validated.lease.source,"relationship_epoch":epoch,"fact":fact,"policy":validated.policy.appraisal.interpretation_digest()?,"roots":validated.lease.dependencies}),
        )?;
        let input = AppraisalInput {
            rebuild: None,
            effective_at: row.try_get("", "updated_at")?,
            lease: validated.lease,
            relationship_epoch: epoch,
            policy: validated.policy.appraisal,
            fact,
            occurrence: format!("response:{response_id}"),
            input_digest,
            text,
            recalls: Vec::new(),
        };
        tx.commit().await?;
        Ok(input)
    }

    /// Only a terminal execution row with this exact original generation may
    /// become an interpretation input. A tool result or model claim is not one.
    pub async fn prepare_execution_appraisal(
        &self,
        binding: &ContextBinding,
        effect_id: Uuid,
    ) -> ContextResult<AppraisalInput> {
        let tx = self.db.begin().await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let validated =
            Self::validate_in_transaction(&tx, binding, ContextUse::AppraisalWrite).await?;
        let epoch = original_relationship_epoch(&tx, &validated.lease).await?;
        let (status, effective_at, projection) =
            execution_fact(&tx, &validated.lease, effect_id).await?;
        let fact = AppraisalFact::Execution {
            effect_id,
            projection_digest: digest(&projection)?,
        };
        let input_digest = digest(
            &json!({"source":validated.lease.source,"relationship_epoch":epoch,"fact":fact,"policy":validated.policy.appraisal.interpretation_digest()?,"roots":validated.lease.dependencies}),
        )?;
        let input = AppraisalInput {
            rebuild: None,
            effective_at,
            lease: validated.lease,
            relationship_epoch: epoch,
            policy: validated.policy.appraisal,
            fact,
            occurrence: format!("execution:{effect_id}"),
            input_digest,
            text: status,
            recalls: Vec::new(),
        };
        tx.commit().await?;
        Ok(input)
    }
}

async fn execution_fact(
    tx: &DatabaseTransaction,
    lease: &ContextLease,
    effect_id: Uuid,
) -> ContextResult<(String, DateTime<Utc>, Value)> {
    let source = &lease.source;
    let row=tx.query_one(stmt("SELECT status,updated_at,source_event_id,source_privacy_epoch,source_received_at FROM execution_effects WHERE effect_id=$1 AND context_id=$2 AND user_id=$3 AND request_id=$4 FOR SHARE",vec![effect_id.into(),lease.id.into(),source.owner_id.into(),source.request_id.into()])).await?.ok_or_else(||ContextError::Revoked("execution does not belong to the original generation".into()))?;
    ensure_current!(
        row.try_get::<Option<Uuid>>("", "source_event_id")? == source.source_event_id
            && row.try_get::<Option<i64>>("", "source_privacy_epoch")?
                == Some(source.source_privacy_epoch)
            && row
                .try_get::<DateTime<Utc>>("", "source_received_at")?
                .timestamp_micros()
                == source.source_received_at.timestamp_micros(),
        "execution source differs from the admitted input"
    );
    let status: String = row.try_get("", "status")?;
    ensure_current!(
        matches!(
            status.as_str(),
            "succeeded" | "failed" | "unknown" | "cancelled"
        ),
        "execution has no terminal fact"
    );
    let projection = execution_projection(tx, source.owner_id, effect_id)
        .await?
        .ok_or_else(|| ContextError::Revoked("execution fact unavailable".into()))?;
    anyhow::ensure!(
        serde_json::to_vec(&projection)?.len() <= MAX_CONTEXT_BYTES,
        "execution fact exceeds appraisal bounds"
    );
    Ok((status, row.try_get("", "updated_at")?, projection))
}

fn stable_id(material: &[u8]) -> Uuid {
    let hash = Sha256::digest(material);
    let mut id = [0; 16];
    id.copy_from_slice(&hash[..16]);
    id[6] = (id[6] & 0x0f) | 0x80;
    id[8] = (id[8] & 0x3f) | 0x80;
    Uuid::from_bytes(id)
}

fn normalized_events(
    input: &AppraisalInput,
    id: Uuid,
    mut drafts: Vec<AppraisalEvent>,
) -> ContextResult<Vec<AppraisalEvent>> {
    anyhow::ensure!(
        drafts.len() <= 64 && serde_json::to_vec(&drafts)?.len() <= 64 * 1024,
        "appraisal draft bounds exceeded"
    );
    drafts.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| format!("{:?}", a.scope).cmp(&format!("{:?}", b.scope)))
    });
    let mut keys = std::collections::BTreeSet::new();
    for (index, event) in drafts.iter_mut().enumerate() {
        anyhow::ensure!(
            !event.kind.is_empty()
                && event.kind.len() <= 64
                && event
                    .kind
                    .bytes()
                    .all(|v| v.is_ascii_lowercase() || v.is_ascii_digit() || v == b'_'),
            "appraisal kind is not a reason code"
        );
        anyhow::ensure!(
            keys.insert((event.kind.clone(), format!("{:?}", event.scope))),
            "duplicate interpretation kind"
        );
        anyhow::ensure!(
            !matches!(event.scope, AppraisalScope::Device | AppraisalScope::Global),
            "user-rooted appraisal cannot acquire global/device scope"
        );
        let d = event.deltas;
        anyhow::ensure!(
            event.confidence.is_finite()
                && (0.0..=1.0).contains(&event.confidence)
                && [
                    d.valence,
                    d.arousal,
                    d.calmness,
                    d.engagement,
                    d.fatigue,
                    d.battery_stress,
                    d.thermal_stress,
                    d.sleepiness,
                    d.interaction_load,
                    d.trust,
                    d.familiarity
                ]
                .iter()
                .all(|v| v.is_finite() && (-1.0..=1.0).contains(v)),
            "invalid appraisal numerical result"
        );
        // PostgreSQL numeric JSONB normalizes -0.0 to +0.0. Canonicalize
        // every draft scalar before hashing so a real round trip is stable.
        let zero = |value: f32| if value == 0.0 { 0.0 } else { value };
        let d = crate::expression::AffectDeltas {
            valence: zero(d.valence),
            arousal: zero(d.arousal),
            calmness: zero(d.calmness),
            engagement: zero(d.engagement),
            fatigue: zero(d.fatigue),
            battery_stress: zero(d.battery_stress),
            thermal_stress: zero(d.thermal_stress),
            sleepiness: zero(d.sleepiness),
            interaction_load: zero(d.interaction_load),
            trust: zero(d.trust),
            familiarity: zero(d.familiarity),
        };
        event.confidence = zero(event.confidence);
        if !input.policy.affect.relationship_persistence_enabled
            && matches!(
                event.scope,
                AppraisalScope::User | AppraisalScope::Relationship
            )
        {
            event.scope = AppraisalScope::Turn;
        }
        let source = match input.fact {
            AppraisalFact::UserMessage { .. } => AppraisalSource::UserMessage,
            AppraisalFact::ResponseCommitted { .. } => AppraisalSource::AssistantResponse,
            _ => AppraisalSource::Runtime,
        };
        *event = AppraisalEvent::new(
            event.kind.clone(),
            source,
            event.target,
            event.confidence,
            event.scope,
            input.effective_at,
            &input.policy.affect,
            d,
            event.kind.clone(),
        );
        event.id = stable_id(format!("{id}:{index}").as_bytes());
    }
    Ok(drafts)
}

pub(crate) async fn validate_fact(
    tx: &DatabaseTransaction,
    lease: &ContextLease,
    fact: &AppraisalFact,
) -> ContextResult<()> {
    match fact {
        AppraisalFact::UserMessage { id, content_digest } => {
            ensure_current!(
                Some(*id) == lease.source.source_event_id,
                "appraisal changed user source"
            );
            let row = tx
                .query_one(stmt(
                    "SELECT content FROM chat_messages WHERE id=$1 FOR SHARE",
                    vec![(*id).into()],
                ))
                .await?
                .ok_or_else(|| ContextError::Revoked("source message missing".into()))?;
            ensure_current!(
                digest(&json!(row.try_get::<String>("", "content")?))? == *content_digest,
                "source message changed"
            );
        }
        AppraisalFact::Recall {
            context_id,
            material_digest,
        } => {
            ensure_current!(
                *context_id == lease.id && *material_digest == lease.materialized_digest,
                "recall materialization changed"
            );
        }
        AppraisalFact::ResponseCommitted {
            response_id,
            content_digest,
        } => {
            let row=tx.query_one(stmt("SELECT final_content FROM delivery_responses WHERE response_id=$1 AND context_id=$2 AND user_id=$3 AND request_id=$4 AND status='committed' FOR SHARE",vec![(*response_id).into(),lease.id.into(),lease.source.owner_id.into(),lease.source.request_id.into()])).await?.ok_or_else(||ContextError::Revoked("response fact unavailable".into()))?;
            ensure_current!(
                digest(&row.try_get::<Value>("", "final_content")?)? == *content_digest,
                "response fact changed"
            );
        }
        AppraisalFact::Execution {
            effect_id,
            projection_digest,
        } => {
            let (_, _, projection) = execution_fact(tx, lease, *effect_id).await?;
            ensure_current!(
                digest(&projection)? == *projection_digest,
                "execution fact changed"
            );
        }
    }
    Ok(())
}

pub(crate) async fn submit(
    tx: &DatabaseTransaction,
    input: AppraisalInput,
    drafts: Vec<AppraisalEvent>,
) -> ContextResult<AppraisalReceipt> {
    if input.rebuild.is_some() {
        ContextService::validate_in_transaction(tx, &input.binding(), ContextUse::AppraisalWrite)
            .await?;
        super::rebuild::verify_claim(tx, &input).await?;
    }
    let claim = input.rebuild.clone();
    let receipt = submit_inner(tx, input, drafts).await?;
    if let Some(claim) = claim {
        super::rebuild::finish_claim(tx, &claim, &receipt).await?;
    }
    Ok(receipt)
}
async fn submit_inner(
    tx: &DatabaseTransaction,
    input: AppraisalInput,
    drafts: Vec<AppraisalEvent>,
) -> ContextResult<AppraisalReceipt> {
    let validated =
        ContextService::validate_in_transaction(tx, &input.binding(), ContextUse::AppraisalWrite)
            .await?;
    anyhow::ensure!(
        original_relationship_epoch(tx, &validated.lease).await? == input.relationship_epoch,
        "appraisal relationship authority changed"
    );
    anyhow::ensure!(
        validated.policy.appraisal == input.policy,
        "appraisal policy changed"
    );
    validate_fact(tx, &validated.lease, &input.fact).await?;
    let source = &input.lease.source;
    let id = input.id();
    let events = normalized_events(&input, id, drafts)?;
    let results = serde_json::to_value(&events)?;
    let result_digest = digest(&results)?;
    if let Some(row)=tx.query_one(stmt("SELECT revision,input_digest,result_digest,events,status FROM appraisal_records WHERE appraisal_id=$1 FOR UPDATE",vec![id.into()])).await? {
        anyhow::ensure!(row.try_get::<String>("", "input_digest")?==input.input_digest && row.try_get::<String>("", "result_digest")?==result_digest,"appraisal identity changed intent");
        anyhow::ensure!(row.try_get::<String>("", "status")?=="active","appraisal identity was retired");
        return Ok(AppraisalReceipt{appraisal_id:id,revision:row.try_get("", "revision")?,result_digest,event_count:events.len()});
    }
    // The record remains evidence while any of its events is live. Consumers
    // separately cap their projection by every event they actually used.
    let lifetime = events
        .iter()
        .map(|event| event.ttl_sec)
        .max()
        .unwrap_or(input.policy.affect.turn_appraisal_ttl_sec);
    let mut until = input.effective_at + chrono::Duration::seconds(lifetime.min(31_536_000) as i64);
    if let Some(parent) = input.lease.valid_until {
        until = until.min(parent);
    }
    ensure_live(tx, Some(until)).await?;
    tx.execute(stmt("UPDATE appraisal_records SET status='superseded',revision=revision+1 WHERE owner_id=$1 AND source_event_id=$2 AND occurrence_key=$3 AND appraisal_id<>$4 AND status='active'",vec![source.owner_id.into(),source.source_event_id.unwrap().into(),input.occurrence.clone().into(),id.into()])).await?;
    let roots = json!(input.lease.dependencies);
    tx.execute(stmt("INSERT INTO appraisal_records(appraisal_id,owner_id,request_id,context_id,occurrence_key,interpreter_id,interpreter_version,policy_version,interpretation_policy_digest,source_event_id,source_privacy_epoch,source_relationship_epoch,source_received_at,input_digest,result_digest,source_fact,events,roots,roots_digest,effective_at,valid_until) VALUES($1,$2,$3,$4,$5,'alia.appraisal',$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)",vec![id.into(),source.owner_id.into(),source.request_id.into(),input.lease.id.into(),input.occurrence.clone().into(),input.policy.interpreter_version.clone().into(),input.lease.policy_version.into(),input.policy.interpretation_digest()?.into(),source.source_event_id.unwrap().into(),source.source_privacy_epoch.into(),input.relationship_epoch.into(),source.source_received_at.into(),input.input_digest.into(),result_digest.clone().into(),json!(input.fact).into(),results.into(),roots.clone().into(),digest(&roots)?.into(),input.effective_at.into(),until.into()])).await?;
    Ok(AppraisalReceipt {
        appraisal_id: id,
        revision: 1,
        result_digest,
        event_count: events.len(),
    })
}

pub(super) fn record_events(row: &QueryResult) -> ContextResult<Vec<AppraisalEvent>> {
    let events: Vec<AppraisalEvent> = serde_json::from_value(row.try_get("", "events")?)?;
    anyhow::ensure!(
        events.len() <= 64
            && digest(&json!(events))? == row.try_get::<String>("", "result_digest")?,
        "appraisal result digest mismatch"
    );
    Ok(events)
}
pub(super) fn record_roots(row: &QueryResult) -> ContextResult<Vec<ContextEvidence>> {
    let roots: Vec<ContextEvidence> = serde_json::from_value(row.try_get("", "roots")?)?;
    check_bounds(&roots)?;
    anyhow::ensure!(
        digest(&json!(roots))? == row.try_get::<String>("", "roots_digest")?,
        "appraisal roots digest mismatch"
    );
    Ok(roots)
}

pub(crate) async fn validate_record_reference(
    tx: &DatabaseTransaction,
    lease: &ContextLease,
    id: Uuid,
    revision: i64,
    result_digest: &str,
) -> ContextResult<()> {
    let row=tx.query_one(stmt("SELECT * FROM appraisal_records WHERE appraisal_id=$1 AND owner_id=$2 AND status='active' FOR SHARE",vec![id.into(),lease.source.owner_id.into()])).await?.ok_or_else(||ContextError::Revoked("appraisal was removed or superseded".into()))?;
    ensure_current!(
        row.try_get::<i64>("", "revision")? == revision
            && row.try_get::<String>("", "result_digest")? == result_digest,
        "appraisal revision changed"
    );
    ensure_current!(
        row.try_get::<i64>("", "source_privacy_epoch")? == lease.source.source_privacy_epoch
            && row.try_get::<i64>("", "source_relationship_epoch")?
                == relationship_epoch(tx, lease.source.owner_id).await?,
        "appraisal authority changed"
    );
    ensure_current!(
        row.try_get::<i64>("", "policy_version")? == lease.policy_version,
        "appraisal requires explicit policy rebuild"
    );
    ensure_live(tx, Some(row.try_get("", "valid_until")?)).await?;
    let roots = record_roots(&row)?;
    ensure_current!(
        roots.iter().all(|root| lease.dependencies.contains(root)),
        "appraisal omitted a consumed root"
    );
    record_events(&row)?;
    let parent_id: Uuid = row.try_get("", "context_id")?;
    validate_appraisal_parent_header(tx, lease, parent_id).await?;
    let parent = load(tx, lease.source.owner_id, parent_id).await?;
    ensure_current!(
        parent.dependencies == roots
            && parent.source.source_event_id == Some(row.try_get("", "source_event_id")?)
            && parent.source.request_id == row.try_get::<Uuid>("", "request_id")?,
        "appraisal parent source differs"
    );
    original_relationship_epoch(tx, &parent).await?;
    validate_fact(
        tx,
        &parent,
        &serde_json::from_value(row.try_get("", "source_fact")?)?,
    )
    .await?;
    Ok(())
}

pub(crate) async fn validate_projection_reference(
    tx: &DatabaseTransaction,
    lease: &ContextLease,
    id: Uuid,
    content_digest: &str,
) -> ContextResult<()> {
    let row = tx
        .query_one(stmt(
            "SELECT * FROM affect_projections WHERE projection_id=$1 AND owner_id=$2 FOR SHARE",
            vec![id.into(), lease.source.owner_id.into()],
        ))
        .await?
        .ok_or_else(|| ContextError::Revoked("affect projection removed".into()))?;
    ensure_current!(
        row.try_get::<i64>("", "policy_version")? == lease.policy_version
            && row.try_get::<i64>("", "source_privacy_epoch")? == lease.source.source_privacy_epoch
            && row.try_get::<i64>("", "relationship_epoch")?
                == relationship_epoch(tx, lease.source.owner_id).await?,
        "affect projection authority changed"
    );
    ensure_live(tx, row.try_get("", "valid_until")?).await?;
    let projection: AffectProjection = serde_json::from_value(row.try_get("", "content")?)?;
    ensure_current!(
        projection.id == id
            && projection.owner == lease.source.owner_id
            && digest(&json!(projection))? == content_digest
            && row.try_get::<String>("", "content_digest")? == content_digest,
        "affect projection digest mismatch"
    );
    let roots = record_roots(&row)?;
    ensure_current!(
        roots.iter().all(|root| lease.dependencies.contains(root)),
        "affect projection lost its roots"
    );
    for appraisal in projection.appraisals {
        ensure_current!(
            roots.contains(&ContextEvidence::Appraisal {
                id: appraisal.appraisal_id,
                revision: appraisal.revision,
                result_digest: appraisal.result_digest
            }),
            "affect projection omitted an interpretation"
        );
    }
    validate_parent_header(tx, lease, row.try_get("", "context_id")?).await?;
    Ok(())
}

impl ContextService {
    /// Extend before output preparation. The caller retains the same turn lock
    /// through this operation, so a response cannot freeze a half-built read set.
    pub async fn extend_affect(
        &self,
        original: &GenerationContext,
    ) -> ContextResult<GenerationContext> {
        let tx = self.db.begin().await?;
        tx.execute(stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let validated =
            Self::validate_in_transaction(&tx, &original.lease.binding(), ContextUse::AffectRead)
                .await?;
        let epoch = original_relationship_epoch(&tx, &validated.lease).await?;
        let policy = validated.policy.appraisal;
        anyhow::ensure!(
            policy.reducer_version == AFFECT_REDUCER_VERSION,
            "configured affect reducer is unavailable"
        );
        let as_of = tx
            .query_one(stmt("SELECT clock_timestamp() AS now", vec![]))
            .await?
            .unwrap()
            .try_get::<DateTime<Utc>>("", "now")?;
        let mut merged = original.clone();
        let parent_id = original.lease.id;
        merged.lease.id = Uuid::new_v4();
        merge(
            &mut merged.lease.dependencies,
            &[ContextEvidence::Relationship { epoch }],
        )?;
        let rows=tx.query_all(stmt("SELECT * FROM appraisal_records WHERE owner_id=$1 AND source_privacy_epoch=$2 AND source_relationship_epoch=$3 AND policy_version=$4 AND status='active' AND effective_at<=$5 AND valid_until>$5 ORDER BY effective_at DESC,appraisal_id LIMIT 256",vec![original.source().owner_id.into(),original.source().source_privacy_epoch.into(),epoch.into(),original.lease.policy_version.into(),as_of.into()])).await?;
        let mut events = Vec::new();
        let mut receipts = Vec::new();
        for row in rows {
            if !policy.affect.enabled {
                break;
            }
            let mut batch = record_events(&row)?;
            let parent = load(
                &tx,
                original.source().owner_id,
                row.try_get("", "context_id")?,
            )
            .await?;
            batch.retain(|event| {
                event.is_active(as_of)
                    && match event.scope {
                        AppraisalScope::User | AppraisalScope::Relationship => true,
                        AppraisalScope::Session => {
                            parent.source.conversation_id == original.source().conversation_id
                        }
                        AppraisalScope::Turn => {
                            parent.source.request_id == original.source().request_id
                        }
                        _ => false,
                    }
            });
            if batch.is_empty()
                || events.len() + batch.len() > policy.affect.max_active_appraisal_events
            {
                continue;
            }
            let batch_until = batch
                .iter()
                .map(|event| {
                    event.created_at
                        + chrono::Duration::seconds(event.ttl_sec.min(31_536_000) as i64)
                })
                .min();
            let id: Uuid = row.try_get("", "appraisal_id")?;
            let revision: i64 = row.try_get("", "revision")?;
            let result_digest: String = row.try_get("", "result_digest")?;
            let mut candidate = merged.lease.clone();
            if merge(&mut candidate.dependencies, &record_roots(&row)?).is_err()
                || merge(
                    &mut candidate.dependencies,
                    &[ContextEvidence::Appraisal {
                        id,
                        revision,
                        result_digest: result_digest.clone(),
                    }],
                )
                .is_err()
            {
                continue;
            }
            // Revoked candidates are omitted in their entirety. SQL failures
            // remain errors; they are never disguised as an empty relationship.
            match validate_evidence(&tx, &candidate).await {
                Ok(()) => (),
                Err(error)
                    if error.downcast_ref::<ContextError>().is_some_and(|e| {
                        matches!(e, ContextError::Revoked(_) | ContextError::PolicyChanged)
                    }) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
            candidate.valid_until =
                earliest(candidate.valid_until, Some(row.try_get("", "valid_until")?));
            candidate.valid_until = earliest(candidate.valid_until, batch_until);
            merged.lease = candidate;
            receipts.push(AppraisalReceipt {
                appraisal_id: id,
                revision,
                result_digest,
                event_count: batch.len(),
            });
            events.extend(batch);
        }
        let (snapshot, mut profile) = crate::expression::affect::reduce_validated_events(
            original.source().owner_id,
            events,
            &policy.affect,
            as_of,
        );
        profile.relationship_epoch = epoch;
        let projection = AffectProjection {
            id: Uuid::new_v4(),
            owner: original.source().owner_id,
            relationship_epoch: epoch,
            policy_version: original.lease.policy_version,
            reducer_version: policy.reducer_version,
            as_of,
            snapshot,
            profile,
            appraisals: receipts,
        };
        let content = json!(projection);
        let content_digest = digest(&content)?;
        let roots = json!(merged.lease.dependencies);
        anyhow::ensure!(
            serde_json::to_vec(&content)?.len() <= 64 * 1024,
            "affect projection exceeds bounds"
        );
        tx.execute(stmt("INSERT INTO affect_projections(projection_id,owner_id,context_id,policy_version,source_privacy_epoch,relationship_epoch,reducer_version,content,content_digest,roots,roots_digest,as_of,valid_until) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",vec![projection.id.into(),projection.owner.into(),parent_id.into(),projection.policy_version.into(),original.source().source_privacy_epoch.into(),epoch.into(),projection.reducer_version.clone().into(),content.clone().into(),content_digest.clone().into(),roots.clone().into(),digest(&roots)?.into(),as_of.into(),merged.lease.valid_until.into()])).await?;
        merge(
            &mut merged.lease.dependencies,
            &[ContextEvidence::AffectProjection {
                id: projection.id,
                content_digest,
            }],
        )?;
        merged.affect = Some(std::sync::Arc::new(projection.clone()));
        merged.extensions.push(json!({"affect":content}));
        let payload = super::materialize::materialized_payload(&merged);
        anyhow::ensure!(
            serde_json::to_vec(&payload)?.len() <= MAX_CONTEXT_BYTES,
            "affect context exceeds material budget"
        );
        validate_evidence(&tx, &merged.lease).await?;
        persist(
            &tx,
            &mut merged.lease,
            &format!("affect:{}", projection.id),
            "generation",
            &payload,
        )
        .await?;
        let id = merged.lease.id;
        tx.commit()
            .await
            .map_err(|_| ContextError::CommitUnknown { context_id: id })?;
        Ok(merged)
    }
}

pub(super) fn is_revoked(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ContextError>().is_some_and(|e| {
        matches!(
            e,
            ContextError::Revoked(_) | ContextError::PolicyChanged | ContextError::BoundsExceeded
        )
    })
}
