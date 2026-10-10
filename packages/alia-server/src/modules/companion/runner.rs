//! One managed attempt retains its immutable intent and original voice producer.
//! Nothing in this runner reconstructs permission from an Existing receipt.
use std::{future::Future, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::live::{
    AdmissionResult, INVITATION_MEDIA_KEY, LiveAdmissionPermit, LiveError, LiveRepository,
    PendingInvitation, RetirementReason,
};
use super::types::CandidateKind;
use crate::{
    channels::adapter::{AudioFormat, AudioPayload, AudioPayloadStream, ChannelTarget},
    modules::{
        delivery::{
            AudioDelivery, DeliveryService,
            types::{DeliveryStatus, ResponseRecord},
        },
        interaction::{
            initiative::{VoiceInitiativeHandle, VoiceInitiativeRequest},
            response::ResponseCommitter,
        },
        memory::context::{ContextBinding, ContextService, ContextUse},
    },
    providers::{
        adapter::{ChatMessage, TtsAdapter, TtsRequest},
        tts::audio_budget::ResponseAudioBudget,
    },
};

const RECONCILIATION_ATTEMPTS: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(100);
const CLEANUP_BUDGET: Duration = Duration::from_secs(2);

/// These dependencies are installed only when the complete live path is ready.
/// Shadow construction has no TTS or output capability.
pub(crate) struct LiveOutputs {
    pub context: Arc<ContextService>,
    pub delivery: Arc<DeliveryService>,
    pub responses: Arc<ResponseCommitter>,
    pub tts: Arc<dyn TtsAdapter>,
}

pub(crate) struct LiveRunner {
    repository: Arc<LiveRepository>,
    voice: VoiceInitiativeHandle,
    outputs: LiveOutputs,
    stop: CancellationToken,
}

#[derive(Debug)]
pub(crate) enum RunOutcome {
    NoOpportunity,
    Existing,
    Retired,
    Observed(DeliveryStatus),
    Failed(&'static str),
}

impl LiveRunner {
    pub(crate) fn new(
        repository: Arc<LiveRepository>,
        voice: VoiceInitiativeHandle,
        outputs: LiveOutputs,
        stop: CancellationToken,
    ) -> Self {
        Self {
            repository,
            voice,
            outputs,
            stop,
        }
    }

    pub(crate) async fn run(&self, pending: PendingInvitation) -> RunOutcome {
        let cancel = self.stop.child_token();
        // Cancellation of this managed task cannot detach an original reserve,
        // synthesis or Delivery. Successful submission leaves its native tail
        // with the existing output owner, not a replacement task here.
        let guard = cancel.clone().drop_guard();
        let snapshot_started = std::time::Instant::now();
        let snapshot = within(
            pending.expires_at,
            &cancel,
            self.voice.snapshot(pending.owner_id, &pending.channel_id),
        )
        .await;
        tracing::debug!(
            phase = "voice_snapshot",
            wait_ms = snapshot_started.elapsed().as_millis() as u64,
            ready = snapshot.is_ok(),
            clock = "server_process_monotonic",
            "Companion admission wait"
        );
        let opportunity = match snapshot {
            Ok(value) => value,
            Err(_) if cancel.is_cancelled() => {
                self.retire_candidate(pending.candidate_id, RetirementReason::Shutdown)
                    .await;
                return RunOutcome::Retired;
            }
            Err(_) => return RunOutcome::NoOpportunity,
        };
        let prepare_started = std::time::Instant::now();
        let prepared = within(pending.expires_at, &cancel, async {
            Ok(self
                .repository
                .prepare_intent(pending.candidate_id, opportunity)
                .await?)
        })
        .await;
        tracing::debug!(
            phase = "freeze_intent",
            wait_ms = prepare_started.elapsed().as_millis() as u64,
            ready = prepared.is_ok(),
            clock = "server_process_monotonic",
            "Companion admission wait"
        );
        let intent = match prepared {
            Ok(value) => value,
            Err(_) => {
                self.retire_candidate(pending.candidate_id, RetirementReason::AttemptFailed)
                    .await;
                return RunOutcome::Failed("freeze_intent");
            }
        };

        for attempt in 0..RECONCILIATION_ATTEMPTS {
            let admission_started = std::time::Instant::now();
            // The existing contract deadline is evaluated in the Server's UTC
            // clock here. This is a budget observation, not a measurement of
            // latency between Server, database or device clocks.
            tracing::debug!(
                phase = "admit",
                attempt,
                remaining_ms = (intent.admit_before() - Utc::now()).num_milliseconds(),
                clock = "server_utc_deadline_evaluation",
                "Companion original admission budget"
            );
            let admission = tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                result = tokio::time::timeout(remaining(intent.admit_before()), self.repository.admit(&intent)) => result,
            };
            let outcome = match &admission {
                Ok(Ok(AdmissionResult::Fresh { .. })) => "fresh",
                Ok(Ok(AdmissionResult::Existing(_))) => "existing",
                Ok(Ok(AdmissionResult::Retired)) => "retired",
                Ok(Ok(AdmissionResult::Deferred(_))) => "deferred",
                Ok(Err(_)) => "error",
                Err(_) => "deadline",
            };
            tracing::debug!(
                phase = "admit",
                attempt,
                outcome,
                wait_ms = admission_started.elapsed().as_millis() as u64,
                clock = "server_process_monotonic",
                "Companion admission wait"
            );
            match admission {
                Ok(Ok(AdmissionResult::Fresh {
                    permit,
                    context,
                    response,
                })) => {
                    let admission_id = permit.admission_id();
                    let deadline = permit.deadline();
                    let result = within(
                        deadline,
                        &cancel,
                        self.output(permit, context, response, cancel.clone()),
                    )
                    .await;
                    match result {
                        Ok(status)
                            if matches!(
                                status,
                                DeliveryStatus::Submitted | DeliveryStatus::Completed
                            ) =>
                        {
                            guard.disarm();
                            return RunOutcome::Observed(status);
                        }
                        _ => {
                            cancel.cancel();
                            self.retire_admission(
                                pending.owner_id,
                                admission_id,
                                if self.stop.is_cancelled() {
                                    RetirementReason::Shutdown
                                } else if Utc::now() >= deadline {
                                    RetirementReason::Expired
                                } else {
                                    RetirementReason::AttemptFailed
                                },
                            )
                            .await;
                            return RunOutcome::Failed("original_output");
                        }
                    }
                }
                Ok(Ok(AdmissionResult::Existing(_))) => {
                    // This attempt never received a Fresh permit, so no output
                    // owner was created. Retire its reconciled charge/Response
                    // without granting a replacement producer or a refund.
                    self.retire_candidate(pending.candidate_id, RetirementReason::AttemptFailed)
                        .await;
                    return RunOutcome::Existing;
                }
                Ok(Ok(AdmissionResult::Retired)) => return RunOutcome::Retired,
                // A frozen attempt cannot swap its opportunity on a later scan.
                // Deferral, transient storage failure and unknown commit retain
                // this exact intent for a small bounded reconciliation window.
                Ok(Ok(AdmissionResult::Deferred(_)))
                | Ok(Err(LiveError::Unavailable(_) | LiveError::CommitUnknown { .. }))
                    if attempt + 1 < RECONCILIATION_ATTEMPTS
                        && Utc::now() < intent.admit_before() =>
                {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(RETRY_DELAY.min(remaining(intent.admit_before()))) => {}
                    }
                }
                _ => break,
            }
        }
        self.retire_candidate(
            pending.candidate_id,
            if self.stop.is_cancelled() {
                RetirementReason::Shutdown
            } else if Utc::now() >= intent.admit_before() {
                RetirementReason::Expired
            } else {
                RetirementReason::AttemptFailed
            },
        )
        .await;
        RunOutcome::Retired
    }

    async fn output(
        &self,
        permit: LiveAdmissionPermit,
        context: ContextBinding,
        response: ResponseRecord,
        cancel: CancellationToken,
    ) -> anyhow::Result<DeliveryStatus> {
        let expected = permit.expected_scope();
        let deadline = permit.deadline();
        anyhow::ensure!(
            response.context_id == Some(context.context_id)
                && response.key.user_id == context.owner_id
                && response.key.request_id == context.request_id,
            "fresh invitation lost its canonical Response"
        );
        let reserved = self
            .voice
            .reserve(VoiceInitiativeRequest::from_permit(permit, cancel.clone()))
            .await?;
        anyhow::ensure!(
            reserved.scope == expected
                && reserved.context.context_id == context.context_id
                && reserved.output.request_id() == context.request_id
                && reserved.output.owner() == context.owner_id
                && reserved.deadline <= deadline,
            "voice reservation changed the admitted output identity"
        );
        let output = reserved.output;
        output.bind(response.clone()).await?;
        let literal = CandidateKind::FollowUpInvitation.text();
        // This uses the ordinary atomic Response/history commit. No new user
        // message, affect inference or automatic memory write is fabricated.
        self.outputs
            .responses
            .commit_response(
                context.owner_id,
                response
                    .conversation_id
                    .ok_or_else(|| anyhow::anyhow!("invitation has no original conversation"))?,
                response.response_id,
                ChatMessage::new("assistant", literal),
            )
            .await?;
        output.wait_open().await?;
        self.outputs
            .context
            .validate(&context, ContextUse::OutputAdmission)
            .await?;

        let budget = ResponseAudioBudget::for_response(response.response_id)?;
        let request = TtsRequest::plain(literal);
        let channel = output.channel();
        let audio: AudioPayloadStream = if channel.capabilities().audio_streaming {
            let stream = self
                .outputs
                .tts
                .synthesize_stream_request(&request, budget)
                .await?;
            Box::pin(stream.map(|chunk| {
                chunk.map(|audio| AudioPayload {
                    data: audio.into_buffer(),
                    sample_rate: 24000,
                    channels: 1,
                    format: AudioFormat::Wav,
                })
            }))
        } else {
            let audio = self
                .outputs
                .tts
                .synthesize_request(&request, budget)
                .await?;
            Box::pin(futures::stream::iter([Ok(AudioPayload {
                data: audio.into_buffer(),
                sample_rate: 24000,
                channels: 1,
                format: AudioFormat::Wav,
            })]))
        };
        // Hold retains this same bounded result/stream. No fallback regenerates
        // it, and Delivery's original media owner controls subsequent polling.
        output.wait_open().await?;
        let record = self
            .outputs
            .delivery
            .deliver_audio(AudioDelivery {
                response_id: response.response_id,
                output_key: INVITATION_MEDIA_KEY.into(),
                content: json!({"text": literal}),
                channel,
                target: ChannelTarget {
                    user_id: context.owner_id.to_string(),
                    voice: Some(expected),
                },
                audio,
                cancel,
            })
            .await?;
        Ok(record.status)
    }

    async fn retire_candidate(&self, id: Uuid, reason: RetirementReason) {
        if !matches!(
            tokio::time::timeout(CLEANUP_BUDGET, self.repository.retire_candidate(id, reason))
                .await,
            Ok(Ok(_))
        ) {
            tracing::warn!(
                "Companion candidate retirement unconfirmed; original identity retained"
            );
        }
    }

    async fn retire_admission(&self, owner: Uuid, id: Uuid, reason: RetirementReason) {
        if !matches!(
            tokio::time::timeout(
                CLEANUP_BUDGET,
                self.repository.retire_admission(owner, id, reason)
            )
            .await,
            Ok(Ok(_))
        ) {
            tracing::warn!("Companion admission retirement unconfirmed; original charge retained");
        }
    }
}

fn remaining(deadline: DateTime<Utc>) -> Duration {
    (deadline - Utc::now()).to_std().unwrap_or_default()
}

async fn within<T>(
    deadline: DateTime<Utc>,
    cancel: &CancellationToken,
    operation: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    anyhow::ensure!(
        Utc::now() < deadline,
        "original invitation deadline elapsed"
    );
    tokio::select! {
        biased;
        _ = cancel.cancelled() => anyhow::bail!("original invitation cancelled"),
        result = tokio::time::timeout(remaining(deadline), operation) => {
            result.map_err(|_| anyhow::anyhow!("original invitation deadline elapsed"))?
        }
    }
}

#[cfg(test)]
mod tests;
