//! Companion observations and erasure references projected from canonical evidence.
use sea_orm::DatabaseTransaction;
use uuid::Uuid;

use super::types::{AppraisalReference, CanonicalObservation};
use crate::modules::memory::context::{
    ContextError, ContextEvidence, ContextLease, ContextResult,
    appraisal_source::{AppraisalReadAuthority, load_user_appraisal_source},
};

pub(crate) struct CompanionSource {
    pub observation: CanonicalObservation,
    /// Original parent identity with every flattened canonical dependency.
    pub lease: ContextLease,
    /// Opaque references for transactional erasure, never copied source text.
    pub erasure_roots: Vec<(&'static str, Uuid)>,
}

pub(crate) async fn load_companion_source(
    tx: &DatabaseTransaction,
    owner: Uuid,
    appraisal_id: Uuid,
    authority: &AppraisalReadAuthority,
) -> ContextResult<CompanionSource> {
    let appraisal = load_user_appraisal_source(tx, owner, appraisal_id, authority).await?;
    let lease = appraisal.lease;
    let source = lease.source();
    let revoked = || ContextError::Revoked("Companion source is unavailable".into());
    let source_event_id = source.source_event_id.ok_or_else(revoked)?;
    let conversation_id = source.conversation_id.ok_or_else(revoked)?;
    let reference = AppraisalReference {
        appraisal_id: appraisal.appraisal_id,
        revision: appraisal.revision,
        result_digest: appraisal.result_digest,
    };
    let mut roots = vec![("appraisal", appraisal_id), ("message", source_event_id)];
    for evidence in lease.dependencies() {
        match evidence {
            ContextEvidence::UserMessage { id, .. }
            | ContextEvidence::MemorySourceMessage { id, .. } => roots.push(("message", *id)),
            ContextEvidence::DerivedMessage {
                id, response_id, ..
            } => {
                roots.push(("message", *id));
                roots.push(("response", *response_id));
            }
            ContextEvidence::Memory { id, .. } => roots.push(("memory", *id)),
            ContextEvidence::Graph { edge_id, .. } => roots.push(("graph", *edge_id)),
            ContextEvidence::Cache { id, .. } => roots.push(("cache", *id)),
            ContextEvidence::Execution { effect_id, .. } => roots.push(("execution", *effect_id)),
            ContextEvidence::Appraisal { id, .. } => roots.push(("appraisal", *id)),
            ContextEvidence::AffectProjection { id, .. } => roots.push(("affect", *id)),
            ContextEvidence::Relationship { .. } => (),
            ContextEvidence::CompanionAdmission { .. }
            | ContextEvidence::CompanionHistory { .. } => {
                return Err(ContextError::Revoked(
                    "an invitation cannot become a new Companion source".into(),
                )
                .into());
            }
        }
    }
    roots.sort_unstable();
    roots.dedup();
    Ok(CompanionSource {
        observation: CanonicalObservation {
            owner_id: owner,
            context_id: lease.id(),
            request_id: source.request_id,
            source_event_id,
            conversation_id,
            channel_id: source.channel_id.clone(),
            source_received_at: source.source_received_at,
            effective_at: appraisal.effective_at,
            privacy_epoch: authority.privacy_epoch,
            relationship_epoch: authority.relationship_epoch,
            context_policy_version: authority.context_policy_version,
            appraisals: vec![reference],
            valid_until: appraisal.valid_until,
        },
        erasure_roots: roots,
        lease,
    })
}
