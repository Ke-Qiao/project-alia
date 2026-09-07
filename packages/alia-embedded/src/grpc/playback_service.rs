use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use alia_core::delivery::VoiceScope;
use alia_proto::playback::{self as proto, playback_frame, playback_server::Playback};
use futures::Stream;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use super::auth::{extract_bearer_token, require_auth, require_auth_token};
use crate::binding::BindingManager;
use crate::module::playback_session::{PlaybackLease, PlaybackSession};

pub struct PlaybackService {
    session: Arc<PlaybackSession>,
    binding: Arc<BindingManager>,
}

impl PlaybackService {
    pub fn new(session: Arc<PlaybackSession>, binding: Arc<BindingManager>) -> Self {
        Self { session, binding }
    }
}

struct PushGuard {
    session: Arc<PlaybackSession>,
    lease: PlaybackLease,
    ended: bool,
    auth_watch: tokio::task::JoinHandle<()>,
}

impl Drop for PushGuard {
    fn drop(&mut self) {
        self.auth_watch.abort();
        if !self.ended {
            if let Err(error) = self.session.cancel(&self.lease) {
                tracing::warn!(%error, "failed to retire interrupted playback Push");
            }
        }
    }
}

#[tonic::async_trait]
impl Playback for PlaybackService {
    type PushStream = Pin<Box<dyn Stream<Item = Result<proto::PlaybackReceipt, Status>> + Send>>;

    async fn push(
        &self,
        request: Request<tonic::Streaming<proto::PlaybackFrame>>,
    ) -> Result<Response<Self::PushStream>, Status> {
        let token = extract_bearer_token(request.metadata())?;
        require_auth_token(&self.binding, &token).await?;
        let mut incoming = request.into_inner();
        let session = self.session.clone();
        let binding = self.binding.clone();
        let output = async_stream::try_stream! {
            let first = tokio::time::timeout(Duration::from_secs(5), incoming.message()).await
                .map_err(|_| Status::deadline_exceeded("playback Start timed out"))??
                .ok_or_else(|| Status::invalid_argument("missing playback Start"))?;
            let scope = decode_scope(first.scope.as_ref())?;
            let Some(playback_frame::Payload::Start(start)) = first.payload else {
                Err(Status::invalid_argument("first playback frame must be Start"))?
            };
            if start.channels != 1 { Err(Status::invalid_argument("playback requires mono PCM16"))?; }
            let lease = session.begin(&scope, &first.playback_id, first.playback_sequence, start.sample_rate_hz)
                .map_err(|error| Status::failed_precondition(error.to_string()))?;
            let watch_session = session.clone();
            let watch_lease = lease.clone();
            let auth_watch = tokio::spawn(async move {
                let mut check = tokio::time::interval(Duration::from_millis(250));
                loop {
                    check.tick().await;
                    if !watch_session.is_current(&watch_lease).unwrap_or(false) { break; }
                    if let Err(error) = require_auth_token(&binding, &token).await {
                        tracing::warn!(%error, "playback authorization expired or was revoked");
                        let _ = watch_session.cancel(&watch_lease);
                        break;
                    }
                }
            });
            let mut guard = PushGuard { session: session.clone(), lease, ended: false, auth_watch };
            let mut sequence = 0_u64;
            let mut accepted = 0_u64;
            yield receipt(&guard.lease, proto::PlaybackState::Accepted, accepted, false);
            let mut check = tokio::time::interval(Duration::from_millis(50));
            loop {
                if !session.is_current(&guard.lease).map_err(internal)? {
                    yield receipt(&guard.lease, proto::PlaybackState::Cancelled, accepted, false);
                    break;
                }
                let message = tokio::select! {
                    value = incoming.message() => value,
                    _ = check.tick() => continue,
                };
                let result = async {
                    let frame = message?.ok_or_else(|| Status::invalid_argument("playback stream closed before End"))?;
                    if decode_scope(frame.scope.as_ref())? != scope || frame.playback_id != guard.lease.playback_id
                        || frame.playback_sequence != guard.lease.playback_sequence {
                        return Err(Status::invalid_argument("playback frame identity changed"));
                    }
                    match frame.payload {
                        Some(playback_frame::Payload::Audio(audio)) => {
                            if audio.sequence != sequence || audio.sample_offset != accepted || audio.pcm_s16le.is_empty()
                                || audio.pcm_s16le.len() % 2 != 0 || audio.pcm_s16le.len() > guard.lease.sample_rate as usize / 10 * 2 {
                                return Err(Status::invalid_argument("invalid playback sequence, sample offset, or PCM frame size"));
                            }
                            let pcm: Vec<i16> = audio.pcm_s16le.chunks_exact(2).map(|sample| i16::from_le_bytes([sample[0], sample[1]])).collect();
                            let count = pcm.len() as u64;
                            let session = session.clone();
                            let lease = guard.lease.clone();
                            let admitted = tokio::task::spawn_blocking(move || session.enqueue(&lease, &pcm))
                                .await.map_err(internal)?.map_err(internal)?;
                            if !admitted { return Ok(proto::PlaybackState::Cancelled); }
                            accepted = accepted.checked_add(count).ok_or_else(|| Status::invalid_argument("playback sample clock overflow"))?;
                            sequence = sequence.checked_add(1).ok_or_else(|| Status::invalid_argument("playback sequence overflow"))?;
                            Ok(proto::PlaybackState::Accepted)
                        }
                        Some(playback_frame::Payload::End(end)) => {
                            if end.total_samples != accepted { return Err(Status::invalid_argument("playback End sample count mismatch")); }
                            // EOF after End is mandatory. No extra media may trail End.
                            if tokio::time::timeout(Duration::from_secs(5), incoming.message()).await
                                .map_err(|_| Status::deadline_exceeded("missing EOF after playback End"))??.is_some() {
                                return Err(Status::invalid_argument("playback frame received after End"));
                            }
                            guard.ended = session.end(&guard.lease).map_err(internal)?;
                            Ok(if guard.ended { proto::PlaybackState::Accepted } else { proto::PlaybackState::Cancelled })
                        }
                        _ => Err(Status::invalid_argument("unexpected playback Start or missing payload")),
                    }
                }.await;
                match result {
                    Ok(state) => {
                        yield receipt(&guard.lease, state, accepted, guard.ended);
                        if guard.ended || state == proto::PlaybackState::Cancelled { break; }
                    }
                    Err(error) => {
                        tracing::warn!(%error, playback_id = %guard.lease.playback_id, "playback Push failed");
                        let _ = session.cancel(&guard.lease);
                        yield receipt(&guard.lease, proto::PlaybackState::Failed, accepted, false);
                        break;
                    }
                }
            }
        };
        Ok(Response::new(Box::pin(output)))
    }

    async fn fence(
        &self,
        request: Request<proto::FenceRequest>,
    ) -> Result<Response<proto::FenceReceipt>, Status> {
        require_auth(&self.binding, &request).await?;
        let request = request.into_inner();
        let scope = decode_scope(request.scope.as_ref())?;
        let applied = self
            .session
            .fence(&scope, request.close_session)
            .map_err(internal)?;
        let active = self.session.active_scope().map_err(internal)?;
        Ok(Response::new(proto::FenceReceipt {
            applied,
            active_scope: active.map(encode_scope),
            session_closed: active.is_none(),
        }))
    }
}

fn receipt(
    lease: &PlaybackLease,
    state: proto::PlaybackState,
    accepted_samples: u64,
    end_accepted: bool,
) -> proto::PlaybackReceipt {
    proto::PlaybackReceipt {
        scope: Some(encode_scope(lease.scope)),
        playback_id: lease.playback_id.clone(),
        playback_sequence: lease.playback_sequence,
        state: state as i32,
        accepted_samples,
        end_accepted,
        submitted_samples: None,
        presented_samples: None,
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
