use super::auth::{extract_bearer_token, require_auth, require_auth_token};
use crate::binding::BindingManager;
use crate::module::playback_session::{PlaybackLease, PlaybackSession};
use alia_core::delivery::{
    ObservationEnd, PLAYBACK_CONTRACT_VERSION, PlaybackIdentity, PlaybackProgress,
    SubmissionBoundary, VoiceScope,
};
use alia_proto::playback::{self as proto, playback_frame, playback_server::Playback};
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
use uuid::Uuid;

pub struct PlaybackService {
    session: Arc<PlaybackSession>,
    binding: Arc<BindingManager>,
}
impl PlaybackService {
    pub fn new(session: Arc<PlaybackSession>, binding: Arc<BindingManager>) -> Self {
        Self { session, binding }
    }
}
pub struct PlaybackOutput {
    incoming: mpsc::Receiver<Result<proto::PlaybackReceipt, Status>>,
    cancel: CancellationToken,
}
impl Stream for PlaybackOutput {
    type Item = Result<proto::PlaybackReceipt, Status>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.incoming.poll_recv(cx)
    }
}
impl Drop for PlaybackOutput {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
struct OwnedLease {
    session: Arc<PlaybackSession>,
    lease: PlaybackLease,
}
impl Drop for OwnedLease {
    fn drop(&mut self) {
        if let Err(error) = self.session.cancel(&self.lease) {
            tracing::error!(%error, "playback lease cleanup failed");
        }
    }
}

#[tonic::async_trait]
impl Playback for PlaybackService {
    type PushStream = PlaybackOutput;
    async fn push(
        &self,
        request: Request<tonic::Streaming<proto::PlaybackFrame>>,
    ) -> Result<Response<Self::PushStream>, Status> {
        let token = extract_bearer_token(request.metadata())?;
        require_auth_token(&self.binding, &token).await?;
        let session = self.session.clone();
        let binding = self.binding.clone();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel(8);
        self.session
            .spawn_push(async move {
                let result = run_push(
                    session,
                    binding,
                    token,
                    request.into_inner(),
                    &tx,
                    worker_cancel,
                )
                .await;
                if let Err(error) = &result {
                    let _ = tx.try_send(Err(Status::internal(error.to_string())));
                }
                result
            })
            .map_err(internal)?;
        Ok(Response::new(PlaybackOutput {
            incoming: rx,
            cancel,
        }))
    }
    async fn fence(
        &self,
        request: Request<proto::FenceRequest>,
    ) -> Result<Response<proto::FenceReceipt>, Status> {
        require_auth(&self.binding, &request).await?;
        let request = request.into_inner();
        let scope = decode_scope(request.scope.as_ref())?;
        let mut final_progress = None;
        let applied = match (&request.playback_id, request.playback_sequence) {
            (None, None) => self
                .session
                .fence(&scope, request.close_session)
                .map_err(internal)?,
            (Some(id), Some(sequence)) if !request.close_session => {
                let identity = PlaybackIdentity {
                    scope,
                    playback_id: Uuid::parse_str(id).map_err(internal)?,
                    playback_sequence: sequence,
                };
                if !identity.is_valid() {
                    return Err(Status::invalid_argument("invalid exact playback identity"));
                }
                let progress = self
                    .session
                    .cancel_observed(&identity, &request.reason)
                    .await
                    .map_err(internal)?;
                final_progress = Some(receipt(&progress));
                true
            }
            _ => {
                return Err(Status::invalid_argument(
                    "exact playback Fence requires both identity fields and forbids close_session",
                ));
            }
        };
        let active = self.session.active_scope().map_err(internal)?;
        Ok(Response::new(proto::FenceReceipt {
            applied,
            final_progress,
            active_scope: active.map(encode_scope),
            session_closed: active.is_none(),
            playback_id: request.playback_id,
            playback_sequence: request.playback_sequence,
        }))
    }
}

async fn run_push(
    session: Arc<PlaybackSession>,
    binding: Arc<BindingManager>,
    token: String,
    mut incoming: tonic::Streaming<proto::PlaybackFrame>,
    tx: &mpsc::Sender<Result<proto::PlaybackReceipt, Status>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let shutdown = session.shutdown_token();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let first = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Ok(()),
        _ = shutdown.cancelled() => return Ok(()),
        first = tokio::time::timeout(Duration::from_secs(5), incoming.message()) => first,
    };
    let first = match first {
        Ok(Ok(Some(first))) => first,
        _ => {
            let _ = tx
                .send(Err(Status::invalid_argument(
                    "missing playback Start within deadline",
                )))
                .await;
            return Ok(());
        }
    };
    let scope = match decode_scope(first.scope.as_ref()) {
        Ok(scope) => scope,
        Err(error) => {
            let _ = tx.send(Err(error)).await;
            return Ok(());
        }
    };
    let Some(playback_frame::Payload::Start(start)) = first.payload else {
        let _ = tx
            .send(Err(Status::invalid_argument(
                "first playback frame must be Start",
            )))
            .await;
        return Ok(());
    };
    if start.channels != 1 || start.contract_version != PLAYBACK_CONTRACT_VERSION {
        let _ = tx
            .send(Err(Status::failed_precondition(
                "requires mono PCM16 and playback contract v2",
            )))
            .await;
        return Ok(());
    }
    let begin_session = session.clone();
    let begin_binding = binding.clone();
    let begin_token = token.clone();
    let runtime = tokio::runtime::Handle::current();
    let begin = tokio::task::spawn_blocking(move || {
        runtime.block_on(begin_binding.with_verified_grpc_auth(
            &begin_token,
            alia_transport::EMBEDDED_CONTROL_SCOPE,
            || {
                begin_session.begin(
                    &scope,
                    &first.playback_id,
                    first.playback_sequence,
                    start.sample_rate_hz,
                )
            },
        ))
    });
    // This worker owns the native admission wait even if its response is dropped.
    let lease = match begin.await? {
        Ok(lease) => lease,
        Err(error) => {
            let _ = tx
                .send(Err(Status::failed_precondition(error.to_string())))
                .await;
            return Ok(());
        }
    };
    let _owner = OwnedLease {
        session: session.clone(),
        lease: lease.clone(),
    };
    let mut next_sequence = 0;

    let mut last: Option<PlaybackProgress> = None;
    let mut last_sent = tokio::time::Instant::now() - Duration::from_secs(1);
    let mut input_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut end_seen = false;
    let mut input_closed = false;
    let mut pending_end = false;
    let mut pending: Option<tokio::task::JoinHandle<anyhow::Result<bool>>> = None;
    let mut progress_tick = tokio::time::interval(Duration::from_millis(20));
    let mut auth_tick = tokio::time::interval(Duration::from_millis(250));
    let outcome = loop {
        {
            let mut snapshot = session.progress(&lease, 0);
            // Heartbeats remain independent of a full media queue. A terminal
            // receipt waits until our native enqueue handle has actually joined.
            if pending.is_some() {
                snapshot.observation_end = None;
                if pending_end {
                    snapshot.end_accepted = false;
                }
            }
            let mut comparable = snapshot.clone();
            if let Some(previous) = &last {
                comparable.receipt_sequence = previous.receipt_sequence;
            }
            if last.as_ref() != Some(&comparable) || last_sent.elapsed() >= Duration::from_secs(1) {
                snapshot = lease.submission.next_snapshot();
                if pending.is_some() {
                    snapshot.observation_end = None;
                    if pending_end {
                        snapshot.end_accepted = false;
                    }
                }
                if let Err(error) = snapshot.validate(last.as_ref(), lease.submission.accepted()) {
                    break Some(ObservationEnd::Unknown {
                        reason: format!("invalid backend progress: {error}"),
                    });
                }
                let terminal = snapshot.observation_end.clone();
                if tokio::time::timeout(Duration::from_secs(1), tx.send(Ok(receipt(&snapshot))))
                    .await
                    .is_err()
                    || tx.is_closed()
                {
                    break Some(ObservationEnd::Unknown {
                        reason: "playback receipt connection closed".into(),
                    });
                }
                last = Some(snapshot);
                last_sent = tokio::time::Instant::now();
                if terminal.is_some() {
                    break None;
                }
            }
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break Some(ObservationEnd::Unknown { reason: "playback observer disconnected".into() }),
            _ = shutdown.cancelled() => break Some(ObservationEnd::Cancelled { reason: "device runtime shutdown".into() }),
            _ = tokio::time::sleep_until(deadline) => break Some(ObservationEnd::Cancelled { reason: "absolute playback observation deadline".into() }),
            result = async { pending.as_mut().expect("guarded").await }, if pending.is_some() => {
                pending.take();
                pending_end = false;
                match result {
                    Ok(Ok(true)) => {},
                    Ok(Ok(false)) => break Some(ObservationEnd::Cancelled { reason: "playback media fence applied".into() }),
                    Ok(Err(error)) => break Some(ObservationEnd::Failed { reason: error.to_string() }),
                    Err(error) => break Some(ObservationEnd::Unknown { reason: format!("playback media worker failed: {error}") }),
                }
            }
            _ = auth_tick.tick() => {
                if !matches!(tokio::time::timeout(Duration::from_secs(1), require_auth_token(&binding, &token)).await, Ok(Ok(()))) {
                    break Some(ObservationEnd::Cancelled { reason: "playback authorization expired or was revoked".into() });
                }
            }
            _ = tokio::time::sleep_until(input_deadline), if !input_closed && pending.is_none() => {
                break Some(ObservationEnd::Failed { reason: "playback input/EOF idle deadline".into() });
            }
            message = incoming.message(), if !input_closed && pending.is_none() => {
                input_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                let frame = match message {
                    Ok(Some(frame)) if !end_seen => frame,
                    Ok(None) if end_seen => {
                        input_closed = true;
                        pending_end = true;
                        let session = session.clone(); let lease = lease.clone();
                        pending = Some(tokio::task::spawn_blocking(move || session.end(&lease)));
                        continue;
                    },
                    Ok(Some(_)) => break Some(ObservationEnd::Failed { reason: "audio or another frame after End".into() }),
                    _ => break Some(ObservationEnd::Unknown { reason: "playback input disconnected before End".into() }),
                };
                if decode_scope(frame.scope.as_ref()).ok() != Some(scope) || frame.playback_id != lease.playback_id
                    || frame.playback_sequence != lease.playback_sequence {
                    break Some(ObservationEnd::Failed { reason: "playback frame identity changed".into() });
                }
                match frame.payload {
                    Some(playback_frame::Payload::Audio(audio)) => {
                        if audio.sequence != next_sequence || audio.sample_offset != lease.submission.accepted()
                            || audio.pcm_s16le.is_empty() || audio.pcm_s16le.len() % 2 != 0
                            || audio.pcm_s16le.len() > lease.sample_rate as usize / 10 * 2 {
                            break Some(ObservationEnd::Failed { reason: "invalid playback sequence, offset or PCM size".into() });
                        }
                        let pcm: Vec<i16> = audio.pcm_s16le.chunks_exact(2).map(|p| i16::from_le_bytes([p[0],p[1]])).collect();
                        let session = session.clone(); let lease = lease.clone();
                        pending = Some(tokio::task::spawn_blocking(move || session.enqueue(&lease, &pcm)));
                        next_sequence += 1;
                    },
                    Some(playback_frame::Payload::End(end)) if end.total_samples == lease.submission.accepted() => end_seen = true,
                    _ => break Some(ObservationEnd::Failed { reason: "unexpected playback payload or End count".into() }),
                }
            }
            _ = progress_tick.tick() => {},
        }
    };
    if let Some(outcome) = outcome {
        if !matches!(outcome, ObservationEnd::Cancelled { .. }) {
            lease.submission.terminate(outcome.clone());
        }
        // Independent control wakes a blocked media admission. Never abort/drop
        // its only JoinHandle and pretend the native worker has exited.
        let retired = session.cancel(&lease);
        if matches!(outcome, ObservationEnd::Cancelled { .. }) && !matches!(retired, Ok(true)) {
            lease.submission.terminate(ObservationEnd::Unknown {
                reason: "playback cancellation was not confirmed".into(),
            });
        }
        if let Some(mut task) = pending {
            let _ = (&mut task).await?;
        }
        let settled = tokio::time::timeout(Duration::from_millis(1500), async {
            while !lease.submission.output_quiescent() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        if settled.is_err() {
            // Closing without final evidence is Unknown to the observer. The
            // native backend still owns its output lease and shutdown joins it.
            tracing::warn!(playback_id = %lease.playback_id, "native playback write did not settle; final observation is unknown");
            retired?;
            return Ok(());
        }
        let final_progress = lease.submission.next_snapshot();
        if last.as_ref().is_none_or(|p| p.observation_end.is_none()) {
            final_progress.validate(last.as_ref(), lease.submission.accepted())?;
            let _ = tokio::time::timeout(
                Duration::from_secs(1),
                tx.send(Ok(receipt(&final_progress))),
            )
            .await;
        }
        retired?;
    }
    Ok(())
}

fn receipt(progress: &PlaybackProgress) -> proto::PlaybackReceipt {
    let (end, reason, state) = match &progress.observation_end {
        None => (
            proto::ObservationEnd::Unspecified,
            String::new(),
            proto::PlaybackState::Accepted,
        ),
        Some(ObservationEnd::SubmissionFinished) => (
            proto::ObservationEnd::SubmissionFinished,
            String::new(),
            proto::PlaybackState::Submitted,
        ),
        Some(ObservationEnd::Cancelled { reason }) => (
            proto::ObservationEnd::Cancelled,
            reason.clone(),
            proto::PlaybackState::Cancelled,
        ),
        Some(ObservationEnd::Failed { reason }) => (
            proto::ObservationEnd::Failed,
            reason.clone(),
            proto::PlaybackState::Failed,
        ),
        Some(ObservationEnd::Unknown { reason }) => (
            proto::ObservationEnd::Unknown,
            reason.clone(),
            proto::PlaybackState::Unknown,
        ),
        Some(ObservationEnd::Rejected { reason }) => (
            proto::ObservationEnd::Rejected,
            reason.clone(),
            proto::PlaybackState::Rejected,
        ),
        Some(ObservationEnd::PresentedFinished) => unreachable!("device has no presentation clock"),
    };
    proto::PlaybackReceipt {
        scope: Some(encode_scope(progress.identity.scope)),
        playback_id: progress.identity.playback_id.to_string(),
        playback_sequence: progress.identity.playback_sequence,
        state: state as i32,
        accepted_samples: progress.accepted_source_frames,
        submitted_samples: progress.submitted_source_frames,
        presented_samples: None,
        end_accepted: progress.end_accepted,
        contract_version: PLAYBACK_CONTRACT_VERSION,
        receipt_sequence: progress.receipt_sequence,
        backend: progress.backend.as_ref().map(|b| proto::SubmissionBackend {
            instance_id: b.instance_id.to_string(),
            boundary: match b.boundary {
                SubmissionBoundary::CpalCallbackBuffer => {
                    proto::SubmissionBoundary::CpalCallbackBuffer
                }
                SubmissionBoundary::AlsaWriteAccepted => {
                    proto::SubmissionBoundary::AlsaWriteAccepted
                }
            } as i32,
            output_sample_rate_hz: b.output_sample_rate_hz,
            output_channels: b.output_channels,
        }),
        observation_end: end as i32,
        reason,
    }
}
fn internal(error: impl std::fmt::Display) -> Status {
    Status::internal(error.to_string())
}
fn decode_scope(scope: Option<&proto::VoiceScope>) -> Result<VoiceScope, Status> {
    let scope = scope.ok_or_else(|| Status::invalid_argument("missing playback voice scope"))?;
    let scope = VoiceScope {
        device_boot_id: Uuid::parse_str(&scope.device_boot_id)
            .map_err(|_| Status::invalid_argument("invalid boot UUID"))?,
        session_id: Uuid::parse_str(&scope.session_id)
            .map_err(|_| Status::invalid_argument("invalid session UUID"))?,
        generation_epoch: scope.generation_epoch,
    };
    if !scope.is_valid() {
        return Err(Status::invalid_argument("invalid playback voice scope"));
    }
    Ok(scope)
}
fn encode_scope(scope: VoiceScope) -> proto::VoiceScope {
    proto::VoiceScope {
        device_boot_id: scope.device_boot_id.to_string(),
        session_id: scope.session_id.to_string(),
        generation_epoch: scope.generation_epoch,
    }
}
#[cfg(test)]
mod tests;
