//! PostgreSQL-owned, immutable provenance for actual materialized model inputs.
//! No adapter snapshot or in-memory fixture can manufacture a persistent lease.
pub(crate) mod appraisal_source;
mod capture;
pub(crate) mod invitation;
mod rebuild;
mod relationship;
pub use rebuild::AppraisalRebuildPage;
pub use relationship::RelationshipClearReceipt;
pub(crate) mod appraisal;
mod appraisal_policy;
pub(crate) mod memory_appraisal;
pub use appraisal::{AffectProjection, AppraisalFact, AppraisalInput, AppraisalReceipt};
pub use appraisal_policy::{
    AFFECT_REDUCER_VERSION, APPRAISAL_INTERPRETER_VERSION, AppraisalPolicy,
};
mod materialize;
pub mod policy;
mod repository;
mod types;
pub use policy::{ContextPolicy, ContextPolicyStore, PolicySnapshot};
pub use types::*;

use crate::providers::adapter::EmbeddingAdapter;
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction, TransactionTrait};
use std::sync::Arc;

#[derive(Clone)]
pub struct ContextService {
    db: DatabaseConnection,
    embedding: Arc<dyn EmbeddingAdapter>,
}
impl ContextService {
    pub fn new(db: DatabaseConnection, embedding: Arc<dyn EmbeddingAdapter>) -> Self {
        Self { db, embedding }
    }
    pub fn policy_store(&self) -> ContextPolicyStore {
        ContextPolicyStore::new(self.db.clone())
    }
    pub async fn initialize_policy(&self, policy: ContextPolicy) -> ContextResult<PolicySnapshot> {
        self.policy_store().initialize_or_load(policy).await
    }
    pub async fn policy_in_transaction(
        tx: &DatabaseTransaction,
        expected_version: Option<i64>,
        purpose: ContextUse,
    ) -> ContextResult<PolicySnapshot> {
        policy::verify(tx, expected_version, purpose).await
    }
    pub async fn validate_in_transaction(
        tx: &DatabaseTransaction,
        binding: &ContextBinding,
        purpose: ContextUse,
    ) -> ContextResult<ValidatedContext> {
        Box::pin(repository::validate_binding(tx, binding, purpose)).await
    }
    /// Recheck a frozen binding before a new model phase. Writes must use the
    /// transaction variant so validation and their side effect commit together.
    pub async fn validate(
        &self,
        binding: &ContextBinding,
        purpose: ContextUse,
    ) -> ContextResult<ValidatedContext> {
        let tx = self.db.begin().await?;
        tx.execute(repository::stmt("SET LOCAL statement_timeout='2s'", vec![]))
            .await?;
        let validated = Self::validate_in_transaction(&tx, binding, purpose).await?;
        tx.commit().await?;
        Ok(validated)
    }
    /// Domain writes which already carry a context ID must still compare their
    /// own original request/source against the returned immutable lease.
    pub async fn validate_lease_in_transaction(
        tx: &DatabaseTransaction,
        owner: uuid::Uuid,
        id: uuid::Uuid,
        purpose: ContextUse,
    ) -> ContextResult<ValidatedContext> {
        Box::pin(repository::validate_lease(tx, owner, id, purpose)).await
    }
}

/// Canonical SHA-256 for independently persisted evidence projections.
pub fn evidence_digest(value: &serde_json::Value) -> ContextResult<String> {
    repository::digest(value)
}
