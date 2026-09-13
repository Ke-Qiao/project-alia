use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use serde_json::json;
use tokio::task::JoinSet;
use uuid::Uuid;

use super::service::{ExecutionService, implementation_digest};
use super::types::*;
use crate::providers::adapter::{ToolContext, ToolExecutor};

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
            _ = scan.tick() => {
                if let Err(error) = fill(&service, &mut tasks, worker_id).await {
                    *service.fault.lock().unwrap() = Some(error.to_string());
                    failure = Some(error);
                    service.stop.cancel();
                }
            }
            _ = service.wake.notified() => {
                if let Err(error) = fill(&service, &mut tasks, worker_id).await {
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

async fn fill(
    service: &Arc<ExecutionService>,
    tasks: &mut JoinSet<anyhow::Result<()>>,
    worker_id: Uuid,
) -> anyhow::Result<()> {
    tokio::time::timeout(DATABASE_DEADLINE, service.repository.recover_expired(256)).await??;
    let available = WORKERS.saturating_sub(tasks.len());
    if available == 0 || service.stop.is_cancelled() {
        return Ok(());
    }
    let candidates = tokio::time::timeout(
        DATABASE_DEADLINE,
        service.repository.pending(available as u32),
    )
    .await??;
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
        if digest != effect.tool_digest
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
        if !effect.arguments.is_object() {
            reject(service, &effect, "tool arguments must be a JSON object").await?;
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
    executor: Arc<dyn ToolExecutor>,
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
    let outcome = if remaining.is_zero() {
        Completion::Failed {
            evidence: json!({"reason": "claim deadline elapsed before external dispatch"}),
        }
    } else {
        let execution =
            AssertUnwindSafe(executor.execute(effect.arguments.clone(), &context)).catch_unwind();
        match tokio::time::timeout(remaining.min(MAX_EXECUTION_TIMEOUT), execution).await {
            Ok(Ok(Ok(mut result))) => {
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
            Ok(Ok(Err(error))) => Completion::Unknown {
                reason: bounded_reason(format!(
                    "adapter error; external outcome unverified: {error}"
                )),
            },
            Ok(Err(_)) => Completion::Unknown {
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
            Ok(Ok(_)) => return Ok(()),
            Ok(Err(error)) => last_error = Some(anyhow::Error::from(error)),
            Err(error) => last_error = Some(anyhow::Error::from(error)),
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    Err(last_error.expect("three failed receipt writes"))
}

fn bounded_reason(reason: String) -> String {
    reason.chars().take(4096).collect()
}
