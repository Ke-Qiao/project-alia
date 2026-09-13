use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::providers::adapter::{ChatMessage, MemoryContextPack};

pub const MAX_CONTEXT_BYTES: usize = 128 * 1024;
pub const MAX_EVIDENCE_BYTES: usize = 64 * 1024;
pub const MAX_EVIDENCE_REFS: usize = 256;
pub const MAX_HISTORY_MESSAGES: usize = 64;
pub const MAX_MEMORY_RESULTS: usize = 12;
pub const MAX_GRAPH_RESULTS: usize = 24;

pub type ContextResult<T> = anyhow::Result<T>;
pub type ContextLeaseId = Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("context policy update requires a currently active administrator")]
    Forbidden,
    #[error("context source or evidence was revoked: {0}")]
    Revoked(String),
    #[error("context policy changed")]
    PolicyChanged,
    #[error("context is unavailable: {0}")]
    Unavailable(String),
    #[error("context exceeds its bounded read set")]
    BoundsExceeded,
    #[error("context commit acknowledgement is unknown: {context_id}")]
    CommitUnknown { context_id: Uuid },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextSource {
    pub owner_id: Uuid,
    pub request_id: Uuid,
    pub channel_id: String,
    pub conversation_id: Option<Uuid>,
    pub source_event_id: Option<Uuid>,
    pub source_received_at: DateTime<Utc>,
    pub source_privacy_epoch: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextBinding {
    pub context_id: Uuid,
    pub owner_id: Uuid,
    pub request_id: Uuid,
    pub source_event_id: Option<Uuid>,
    pub source_privacy_epoch: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextUse {
    ResponseCommit,
    OutputAdmission,
    AutomaticMemoryWrite,
    AiMemoryWrite,
    ManualMemoryWrite,
    MemoryMaintenance,
    MemoryEmbedding,
    CacheWrite,
    CacheReuse,
    CacheReview,
    CacheEmbedding,
    Execution,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextPurpose {
    Generation,
    SourceNotice,
    MemoryCapture,
    MemoryMaintenance,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceNotice {
    ProcessingFailedZh,
    ProcessingFailedEn,
}
impl SourceNotice {
    pub fn text(self) -> &'static str {
        match self {
            Self::ProcessingFailedZh => "处理请求时出错了，请稍后重试。",
            Self::ProcessingFailedEn => "The request could not be completed. Please try again.",
        }
    }
}

/// References are built from canonical SQL rows, never accepted from model JSON.
/// The graph digest covers its actual labels and evidence, not an invented clock.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextEvidence {
    Memory {
        id: Uuid,
        revision: i64,
    },
    UserMessage {
        id: Uuid,
        session_id: Uuid,
        source_privacy_epoch: i64,
        content_digest: String,
    },
    DerivedMessage {
        id: Uuid,
        response_id: Uuid,
        context_id: Uuid,
        content_digest: String,
    },
    Graph {
        edge_id: Uuid,
        projection_digest: String,
    },
    Cache {
        id: Uuid,
        revision: i64,
        context_id: Uuid,
        scope: String,
        query_digest: String,
        content_digest: String,
    },
    Execution {
        effect_id: Uuid,
        intent_digest: String,
        completion_digest: String,
    },
}

#[derive(Clone, Debug)]
pub struct ContextLease {
    pub(super) id: Uuid,
    pub(super) purpose: ContextPurpose,
    pub(super) materialized_digest: String,
    pub(super) source: ContextSource,
    pub(super) policy_version: i64,
    pub(super) dependencies: Vec<ContextEvidence>,
    pub(super) valid_until: Option<DateTime<Utc>>,
}
impl ContextLease {
    pub(super) fn authorizes(&self, usage: ContextUse) -> bool {
        match self.purpose {
            ContextPurpose::Generation => true,
            ContextPurpose::SourceNotice => usage == ContextUse::OutputAdmission,
            ContextPurpose::MemoryCapture => usage == ContextUse::AutomaticMemoryWrite,
            ContextPurpose::MemoryMaintenance => usage == ContextUse::MemoryMaintenance,
        }
    }
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn purpose(&self) -> ContextPurpose {
        self.purpose
    }
    pub fn materialized_digest(&self) -> &str {
        &self.materialized_digest
    }
    pub fn notice_text(&self) -> Option<&'static str> {
        if self.purpose != ContextPurpose::SourceNotice {
            return None;
        }
        [
            SourceNotice::ProcessingFailedZh,
            SourceNotice::ProcessingFailedEn,
        ]
        .into_iter()
        .find(|notice| {
            super::repository::digest(&serde_json::json!({"text":notice.text()}))
                .ok()
                .as_deref()
                == Some(self.materialized_digest.as_str())
        })
        .map(SourceNotice::text)
    }
    pub fn source(&self) -> &ContextSource {
        &self.source
    }
    pub fn policy_version(&self) -> i64 {
        self.policy_version
    }
    pub fn dependencies(&self) -> &[ContextEvidence] {
        &self.dependencies
    }
    pub fn valid_until(&self) -> Option<DateTime<Utc>> {
        self.valid_until
    }
    pub fn binding(&self) -> ContextBinding {
        ContextBinding {
            context_id: self.id,
            owner_id: self.source.owner_id,
            request_id: self.source.request_id,
            source_event_id: self.source.source_event_id,
            source_privacy_epoch: self.source.source_privacy_epoch,
        }
    }
    pub fn memory_write_context(
        &self,
        mut snapshot: crate::providers::adapter::MemoryWriteContext,
        origin: crate::providers::adapter::MemoryWriteOrigin,
    ) -> ContextResult<crate::providers::adapter::MemoryWriteContext> {
        use crate::providers::adapter::MemoryWriteOrigin;
        let usage = match origin {
            MemoryWriteOrigin::Manual => ContextUse::ManualMemoryWrite,
            MemoryWriteOrigin::Automatic => ContextUse::AutomaticMemoryWrite,
            MemoryWriteOrigin::Ai => ContextUse::AiMemoryWrite,
            MemoryWriteOrigin::Maintenance => ContextUse::MemoryMaintenance,
        };
        if !self.authorizes(usage) {
            return Err(ContextError::Revoked(
                "context purpose does not authorize this memory origin".into(),
            )
            .into());
        }
        anyhow::ensure!(
            snapshot.user_id == self.source.owner_id
                && snapshot.privacy_epoch == self.source.source_privacy_epoch,
            "memory snapshot differs from original context authority"
        );
        if let Some(version) = snapshot.policy_version {
            anyhow::ensure!(version == self.policy_version, "memory policy changed");
        }
        snapshot.policy_version = Some(self.policy_version);
        snapshot.context_id = Some(self.id);
        snapshot.origin = origin;
        snapshot.source_received_at = self.source.source_received_at;
        snapshot.exact_source_received_at = self
            .source
            .source_event_id
            .map(|id| (id, self.source.source_received_at));
        snapshot.source_event_ids = self.source.source_event_id.into_iter().collect();
        snapshot.source_memories.clear();
        for reference in &self.dependencies {
            match reference {
                ContextEvidence::Memory { id, revision } => {
                    snapshot.source_memories.push((id.to_string(), *revision))
                }
                ContextEvidence::UserMessage { id, .. } => snapshot.source_event_ids.push(*id),
                _ => (),
            }
        }
        snapshot.source_event_ids.sort_unstable();
        snapshot.source_event_ids.dedup();
        snapshot.source_memories.sort_unstable();
        snapshot.source_memories.dedup();
        Ok(snapshot)
    }
}

#[derive(Clone, Debug)]
pub struct ContextHistoryMessage {
    pub(super) id: Uuid,
    pub(super) message: ChatMessage,
}
impl ContextHistoryMessage {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn message(&self) -> &ChatMessage {
        &self.message
    }
}

#[derive(Clone, Debug)]
pub struct GenerationContext {
    pub(super) policy: super::ContextPolicy,
    pub(super) lease: ContextLease,
    pub(super) source_text: String,
    pub(super) extensions: Vec<serde_json::Value>,
    pub(super) memory: MemoryContextPack,
    pub(super) prior_memory: Vec<MemoryContextPack>,
    pub(super) history: Vec<ContextHistoryMessage>,
    pub(super) memory_prompt: String,
}

#[derive(Clone, Debug)]
pub struct CapturedMemoryContext {
    pub(super) lease: ContextLease,
    pub(super) messages: Vec<ContextHistoryMessage>,
    pub(super) memories: Vec<crate::providers::adapter::MemoryNode>,
}
impl CapturedMemoryContext {
    pub fn lease(&self) -> &ContextLease {
        &self.lease
    }
    pub fn messages(&self) -> &[ContextHistoryMessage] {
        &self.messages
    }
    pub fn memories(&self) -> &[crate::providers::adapter::MemoryNode] {
        &self.memories
    }
}
impl GenerationContext {
    pub fn policy(&self) -> &super::ContextPolicy {
        &self.policy
    }
    pub fn lease(&self) -> &ContextLease {
        &self.lease
    }
    pub fn source(&self) -> &ContextSource {
        self.lease.source()
    }
    pub fn memory(&self) -> &MemoryContextPack {
        &self.memory
    }
    /// All memory actually consumed by earlier and current model phases.
    /// Repeated canonical snippets retain the longest prefix actually read.
    pub fn all_materialized_memory(&self) -> MemoryContextPack {
        let mut combined = MemoryContextPack::default();
        for pack in self
            .prior_memory
            .iter()
            .chain(std::iter::once(&self.memory))
        {
            for (out, input) in [
                (&mut combined.user_preferences, &pack.user_preferences),
                (&mut combined.project_context, &pack.project_context),
                (&mut combined.relevant_episodes, &pack.relevant_episodes),
                (&mut combined.affective_guidance, &pack.affective_guidance),
                (&mut combined.boundaries, &pack.boundaries),
            ] {
                for item in input {
                    if let Some(old) = out.iter_mut().find(|old| old.id == item.id) {
                        if item.content.len() > old.content.len() {
                            *old = item.clone();
                        }
                    } else {
                        out.push(item.clone());
                    }
                }
            }
            for item in &pack.graph_context {
                if !combined.graph_context.iter().any(|old| {
                    old.source == item.source
                        && old.relation == item.relation
                        && old.target == item.target
                }) {
                    combined.graph_context.push(item.clone());
                }
            }
            for item in &pack.recall_events {
                if let Some(old) = combined
                    .recall_events
                    .iter_mut()
                    .find(|old| old.memory_id == item.memory_id)
                {
                    if item.content.len() > old.content.len() {
                        *old = item.clone();
                    }
                } else {
                    combined.recall_events.push(item.clone());
                }
            }
            for item in &pack.retrieval_trace {
                if !combined
                    .retrieval_trace
                    .iter()
                    .any(|old| old.memory_id == item.memory_id)
                {
                    combined.retrieval_trace.push(item.clone());
                }
            }
        }
        combined
    }
    pub fn history(&self) -> &[ContextHistoryMessage] {
        &self.history
    }
    pub fn history_messages(&self) -> Vec<ChatMessage> {
        self.history
            .iter()
            .map(|item| item.message.clone())
            .collect()
    }
    pub fn memory_prompt(&self) -> &str {
        &self.memory_prompt
    }
}

#[derive(Clone, Debug)]
pub struct ValidatedContext {
    pub lease: ContextLease,
    pub policy: super::ContextPolicy,
}

#[derive(Clone, Debug)]
pub struct CacheIdentity {
    pub cache_id: Uuid,
    pub revision: i64,
    pub context_id: Uuid,
    pub scope: String,
}
