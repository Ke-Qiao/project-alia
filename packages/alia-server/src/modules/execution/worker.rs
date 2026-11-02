use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use serde_json::json;
use tokio::task::JoinSet;
use uuid::Uuid;

use super::service::{ExecutionService, implementation_digest};
use super::types::*;
use crate::providers::adapter::{ToolContext, ToolEntry};

const WORKERS: usize = 4;
const DATABASE_DEADLINE: Duration = Duration::from_secs(2);

/// Owns every dispatched task until its completion has been persisted (or a
/// persistence failure has been reported to the host). Never aborts on shutdown.
pub(super) async fn run(service: Arc<ExecutionService>) -> anyhow::Result<()> {
    let mut tasks = JoinSet::new();
    let worker_id = Uuid::new_v4();
    let mut scan = tokio::time::interval(Duration::from_millis(500));
    scan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut failure = None;
    loop {
        tokio::select! {
            biased;
            _ = service.stop.cancelled() => break,
            result = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(result) = result {
                    if let Err(error) = result.map_err(anyhow::Error::from).and_then(|r| r) {
                        *service.fault.lock().unwrap() = Some(error.to_string());
                        failure = Some(error);
                        service.stop.cancel();
                    }
                }
            }
            _ = async { tokio::select! { _ = scan.tick() => {}, _ = service.wake.notified() => {} } } => {
                let candidates = match scan_pending(&service, WORKERS.saturating_sub(tasks.len())).await {
                    Ok(candidates) => {
                        *service.scan_fault.lock().unwrap() = None;
                        candidates
                    }
                    Err(error) => {
                        tracing::warn!(%error, "execution ledger scan failed; next scan will reconcile");
                        *service.scan_fault.lock().unwrap() = Some(error.to_string());
                        continue;
                    }
                };
                if let Err(error) = fill(&service, &mut tasks, worker_id, candidates).await {
                    *service.fault.lock().unwrap() = Some(error.to_string());
                    failure = Some(error);
                    service.stop.cancel();
                }
            }
        }
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result.map_err(anyhow::Error::from).and_then(|r| r) {
            failure.get_or_insert(error);
        }
    }
    if let Some(error) = failure {
        *service.fault.lock().unwrap() = Some(error.to_string());
        return Err(error);
    }
    Ok(())
}

async fn scan_pending(
    service: &ExecutionService,
    available: usize,
) -> anyhow::Result<Vec<EffectRecord>> {
    // These transactions only read intents or retire expired claims as unknown;
    // repeating them never grants permission to dispatch an external effect.
    tokio::time::timeout(DATABASE_DEADLINE, service.repository.recover_expired(256)).await??;
    if available == 0 || service.stop.is_cancelled() {
        return Ok(Vec::new());
    }
    Ok(tokio::time::timeout(
        DATABASE_DEADLINE,
        service.repository.pending(available as u32),
    )
    .await??)
}

async fn fill(
    service: &Arc<ExecutionService>,
    tasks: &mut JoinSet<anyhow::Result<()>>,
    worker_id: Uuid,
    candidates: Vec<EffectRecord>,
) -> anyhow::Result<()> {
    for effect in candidates {
        if service.stop.is_cancelled() {
            break;
        }
        if tokio::time::timeout(DATABASE_DEADLINE, service.cancelled_before_claim(&effect))
            .await??
        {
            continue;
        }
        let catalog_guard =
            tokio::time::timeout(DATABASE_DEADLINE, service.catalog_gate.read()).await?;
        // Resolve an Arc once. A registration replacement after this point
        // cannot change the implementation this claim will invoke.
        let executor = tokio::time::timeout(
            DATABASE_DEADLINE,
            service.raw.resolve_executor(&effect.tool_name),
        )
        .await??;
        let policy = service.policy.read().await.clone();
        let Some(prepared) = executor else {
            reject(service, &effect, "tool implementation unavailable").await?;
            continue;
        };
        let definition = prepared.executor.definition();
        let digest = match implementation_digest(&prepared, &service.build_digest) {
            Ok(digest) => digest,
            Err(error) => {
                reject(service, &effect, &error.to_string()).await?;
                continue;
            }
        };
        if !prepared.executor.allows_user(effect.key.user_id)
            || digest != effect.tool_digest
            || policy.digest() != effect.policy_digest
            || !policy.allows(&definition)
        {
            reject(
                service,
                &effect,
                "tool or execution policy changed before dispatch",
            )
            .await?;
            continue;
        }
        if let Err(error) = prepared.executor.validate(&effect.arguments) {
            reject(service, &effect, &error.to_string()).await?;
            continue;
        }
        if service.stop.is_cancelled() {
            break;
        }
        // This await is bounded. An ambiguous commit does not cause an external
        // call: its durable dispatched lease eventually becomes unknown.
        let deadline = tokio::time::Instant::now() + policy.timeout;
        let claim = tokio::time::timeout(
            DATABASE_DEADLINE,
            service.repository.claim(ClaimRequest {
                effect_id: effect.effect_id,
                worker_id,
                expected_tool_digest: digest,
                expected_policy_digest: policy.digest(),
                execution_timeout: policy.timeout,
            }),
        )
        .await??;
        drop(catalog_guard);
        service.forget_cancellation(effect.effect_id).await;
        if let Some(claim) = claim {
            let service = service.clone();
            let executor = prepared.executor;
            tasks.spawn(async move { execute_claim(service, executor, claim, deadline).await });
        }
    }
    Ok(())
}

async fn reject(
    service: &ExecutionService,
    effect: &EffectRecord,
    reason: &str,
) -> anyhow::Result<()> {
    tokio::time::timeout(
        DATABASE_DEADLINE,
        service
            .repository
            .reject_undispatched(effect.effect_id, &effect.intent_digest, reason),
    )
    .await??;
    service.forget_cancellation(effect.effect_id).await;
    Ok(())
}

async fn execute_claim(
    service: Arc<ExecutionService>,
    executor: Arc<ToolEntry>,
    claim: ExecutionClaim,
    deadline: tokio::time::Instant,
) -> anyhow::Result<()> {
    let effect = &claim.effect;
    let context = ToolContext {
        context_id: effect.context_id,
        user_id: effect.key.user_id,
        request_id: effect.key.request_id,
        source_received_at: effect.source_received_at,
        source_event_id: effect.source_event_id,
        source_privacy_epoch: effect.source_privacy_epoch,
        invocation_key: effect.key.invocation_key.clone(),
        conversation_id: effect.conversation_id,
        source: effect.source.clone(),
        channel_id: effect.channel_id.clone(),
        cancel: None,
    };
    // The lease is durable on the database clock; local execution uses the
    // stricter monotonic deadline started before claim, avoiding clock skew.
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let mut pending = None;
    let outcome = if remaining.is_zero() {
        Completion::Failed {
            evidence: json!({"reason": "claim deadline elapsed before external dispatch"}),
        }
    } else {
        let owner = executor.clone();
        let arguments = effect.arguments.clone();
        let mut execution = tokio::spawn(async move {
            AssertUnwindSafe(owner.execute(arguments, &context))
                .catch_unwind()
                .await
        });
        let result =
            tokio::time::timeout(remaining.min(MAX_EXECUTION_TIMEOUT), &mut execution).await;
        if result.is_err() {
            pending = Some(execution);
        }
        match result {
            Ok(Ok(Ok(Ok(mut result)))) => {
                result.call_id = effect.model_call_id.clone();
                let definitive_failure = executor.failure_is_definitive(&result);
                let receipt = serde_json::to_value(result)?;
                if serde_json::to_vec(&receipt)?.len() > MAX_RECEIPT_BYTES {
                    Completion::Unknown {
                        reason: "adapter receipt exceeded durable result limit".into(),
                    }
                } else if receipt["success"] == true {
                    Completion::Succeeded { receipt }
                } else if definitive_failure {
                    Completion::Failed { evidence: receipt }
                } else {
                    Completion::Unknown {
                        reason: bounded_reason(format!(
                            "adapter reported failure without evidence of non-execution: {receipt}"
                        )),
                    }
                }
            }
            Ok(Ok(Ok(Err(error)))) => Completion::Unknown {
                reason: bounded_reason(format!(
                    "adapter error; external outcome unverified: {error}"
                )),
            },
            Ok(Err(_)) | Ok(Ok(Err(_))) => Completion::Unknown {
                reason: "adapter panicked after claim".into(),
            },
            Err(_) => Completion::Unknown {
                reason: "execution deadline exceeded; external outcome unverified".into(),
            },
        }
    };
    let outcome = if serde_json::to_vec(&outcome)?.len() > MAX_RECEIPT_BYTES {
        Completion::Unknown {
            reason: "adapter outcome exceeded durable result limit".into(),
        }
    } else {
        outcome
    };
    // Retry only writing the same receipt, never execution. The deadline's 10s
    // recovery grace leaves room for these bounded acknowledgement retries.
    let mut last_error = None;
    for attempt in 0..3 {
        match tokio::time::timeout(
            DATABASE_DEADLINE,
            service.repository.complete(claim.claim_id, outcome.clone()),
        )
        .await
        {
            Ok(Ok(_)) => {
                // A deadline ends the observer, not ownership of an already
                // dispatched call. Retain its task and slot until it really ends.
                if let Some(pending) = pending {
                    let _ = pending.await;
                }
                return Ok(());
            }
            Ok(Err(error)) => last_error = Some(anyhow::Error::from(error)),
            Err(error) => last_error = Some(anyhow::Error::from(error)),
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    if let Some(pending) = pending {
        let _ = pending.await;
    }
    Err(last_error.expect("three failed receipt writes"))
}

fn bounded_reason(reason: String) -> String {
    reason.chars().take(4096).collect()
}
