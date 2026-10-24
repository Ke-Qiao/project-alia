//! Signed, bounded control requests. Native work belongs to MotionRuntime;
//! cancelling an RPC only stops observation, never drops its device owner.
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use alia_core::embodiment::motion::*;
use alia_proto::motion::{SignedMotionPayload, motion_control_server::MotionControl};
use alia_transport::{motion_reply_payload, sign_payload};
use anyhow::{Result, ensure};
use ed25519_dalek::SigningKey;
use tonic::{Request, Response, Status};

use super::auth::extract_bearer_token;
use crate::{
    binding::BindingManager,
    module::{
        motion::{MotionAuthorization, MotionRuntime},
        playback_session::PlaybackSession,
    },
};

pub struct EmbeddedMotionService {
    binding: Arc<BindingManager>,
    motion: Arc<MotionRuntime>,
    playback: Arc<PlaybackSession>,
    device_id: String,
    signing_key: SigningKey,
}

impl EmbeddedMotionService {
    pub fn new(
        binding: Arc<BindingManager>,
        motion: Arc<MotionRuntime>,
        playback: Arc<PlaybackSession>,
        device_id: String,
        signing_key: SigningKey,
    ) -> Self {
        Self {
            binding,
            motion,
            playback,
            device_id,
            signing_key,
        }
    }

    fn validate_request(&self, request: &MotionRequest) -> Result<()> {
        ensure!(
            request.version == MOTION_CONTRACT_VERSION,
            "unsupported motion protocol"
        );
        ensure!(
            !request.request_id.is_nil() && request.device_id == self.device_id,
            "motion request target or nonce mismatch"
        );
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        ensure!(
            now.abs_diff(u128::from(request.issued_at_unix_ms)) <= 30_000,
            "motion request timestamp expired"
        );
        match &request.operation {
            MotionOperation::Inspect => {}
            MotionOperation::Open {
                device_boot_id,
                server_instance_id,
                ..
            } => ensure!(
                !device_boot_id.is_nil() && !server_instance_id.is_nil(),
                "invalid motion control identity"
            ),
            MotionOperation::Submit { command } => command.validate()?,
            MotionOperation::Observe { identity, .. } => {
                ensure!(identity.is_valid(), "invalid motion observation identity")
            }
            MotionOperation::Cancel { command, reason } => {
                command.validate()?;
                validate_reason(reason)?;
            }
            MotionOperation::Close { lease, reason } => {
                ensure!(lease.is_valid(), "invalid motion close identity");
                validate_reason(reason)?;
            }
        }
        Ok(())
    }

    async fn authenticated<T>(
        &self,
        token: &str,
        envelope: &SignedMotionPayload,
        request: &MotionRequest,
        admit: impl FnOnce(Instant) -> Result<T>,
    ) -> Result<T> {
        self.binding
            .with_verified_motion(
                token,
                &envelope.payload,
                &envelope.signature,
                |expires_at| {
                    self.validate_request(request)?;
                    admit(expires_at)
                },
            )
            .await
    }

    fn renew_if_current(
        &self,
        identity: &MotionCommandIdentity,
        expires_at: Instant,
    ) -> Result<()> {
        if self.motion.control_state().lease.as_ref() == Some(&identity.lease) {
            self.motion
                .renew_control(&identity.lease, MotionAuthorization { expires_at })?;
        }
        Ok(())
    }

    async fn dispatch(
        &self,
        token: &str,
        envelope: &SignedMotionPayload,
        request: &MotionRequest,
    ) -> Result<MotionEvidence> {
        match &request.operation {
            MotionOperation::Inspect => {
                self.authenticated(token, envelope, request, |_| {
                    Ok(MotionEvidence::Control {
                        state: self.motion.control_state(),
                    })
                })
                .await
            }
            MotionOperation::Open {
                device_boot_id,
                expected_generation,
                server_instance_id,
            } => {
                self.authenticated(token, envelope, request, |expires_at| {
                    let before = self.motion.control_state();
                    ensure!(
                        before.device_boot_id == *device_boot_id,
                        "device boot changed before opening control"
                    );
                    let lease = self.motion.open_control(
                        *expected_generation,
                        *server_instance_id,
                        MotionAuthorization { expires_at },
                    )?;
                    Ok(MotionEvidence::Control {
                        state: MotionControlState {
                            device_boot_id: lease.device_boot_id,
                            generation: lease.generation,
                            lease: Some(lease),
                            available: before.available,
                        },
                    })
                })
                .await
            }
            MotionOperation::Submit { command } => {
                // Authenticate before bounded file preparation, then again at
                // actual synchronous admission. Preparation never writes hardware.
                self.authenticated(token, envelope, request, |_| Ok(()))
                    .await?;
                let canonical_command = serde_json::to_vec(command)?;
                let preparation = tokio::time::timeout(
                    Duration::from_secs(2),
                    self.motion
                        .prepare(command.clone(), canonical_command.clone()),
                )
                .await
                .map_err(|_| anyhow::anyhow!("motion preparation confirmation timed out"))
                .and_then(|result| result);
                let admission = match preparation {
                    Ok(prepared) => {
                        self.authenticated(token, envelope, request, |expires_at| {
                            let submit = || {
                                self.motion
                                    .submit(prepared, MotionAuthorization { expires_at })
                            };
                            match command.scope.voice() {
                                Some(scope) => self.playback.with_voice_admission(&scope, submit),
                                None => submit(),
                            }
                        })
                        .await
                    }
                    Err(error) => Err(error),
                };
                let observation = match admission {
                    Ok(observation) => observation,
                    Err(error) => {
                        tracing::debug!(request_id=%request.request_id, error=%error, "motion admission failed; checking exact rejection evidence");
                        // The same Motion gate either returns existing facts or
                        // retires a provably unaccepted sequence. A cache miss
                        // from an old lease cannot be turned into a zero-write
                        // claim. Late preparation can never revive this identity.
                        self.authenticated(token, envelope, request, |_| {
                            self.motion.reject(
                                command.clone(),
                                canonical_command,
                                "motion command was refused before native admission",
                            )
                        })
                        .await?
                    }
                };
                Ok(MotionEvidence::Progress {
                    progress: observation.snapshot(),
                })
            }
            MotionOperation::Observe {
                identity,
                after_receipt_sequence,
            } => {
                let mut observation = self
                    .authenticated(token, envelope, request, |expires_at| {
                        self.renew_if_current(identity, expires_at)?;
                        self.motion.observe(*identity)
                    })
                    .await?;
                let snapshot = observation.snapshot();
                ensure!(
                    snapshot.receipt_sequence >= *after_receipt_sequence,
                    "motion observation cursor exceeds device evidence"
                );
                if snapshot.receipt_sequence == *after_receipt_sequence && snapshot.end.is_none() {
                    let _ = tokio::time::timeout(Duration::from_millis(750), observation.changed())
                        .await;
                }
                // Revocation may have happened while waiting; the original
                // command facts remain queryable, without opening a new writer.
                Ok(MotionEvidence::Progress {
                    progress: observation.snapshot(),
                })
            }
            MotionOperation::Cancel { command, reason } => {
                let identity = command.identity;
                let canonical = serde_json::to_vec(command)?;
                self.authenticated(token, envelope, request, |expires_at| {
                    self.renew_if_current(&identity, expires_at)?;
                    // A cancellation can overtake Submit's file preparation.
                    // Retire an unaccepted sequence before releasing this gate;
                    // existing intent returns its original observation unchanged.
                    self.motion
                        .reject(command.clone(), canonical, reason)
                        .map(|_| ())
                })
                .await?;
                let progress = self.motion.cancel(identity, reason).await?;
                ensure!(
                    progress.identity == identity && progress.writer_quiescent,
                    "motion cancellation has no exact quiescent checkpoint"
                );
                Ok(MotionEvidence::Progress { progress })
            }
            MotionOperation::Close { lease, reason } => {
                self.authenticated(token, envelope, request, |_| {
                    self.motion.close_control(lease, reason)?;
                    // Closing admission is not evidence that an in-flight native
                    // write or physical motor stopped. Query each known command.
                    Ok(MotionEvidence::Control {
                        state: self.motion.control_state(),
                    })
                })
                .await
            }
        }
    }
}

#[tonic::async_trait]
impl MotionControl for EmbeddedMotionService {
    async fn exchange(
        &self,
        request: Request<SignedMotionPayload>,
    ) -> std::result::Result<Response<SignedMotionPayload>, Status> {
        let token = extract_bearer_token(request.metadata())?;
        let envelope = request.into_inner();
        if envelope.payload.len() > MAX_MOTION_PAYLOAD_BYTES || envelope.signature.len() != 64 {
            return Err(Status::invalid_argument(
                "motion payload or signature exceeds protocol bounds",
            ));
        }
        let request: MotionRequest = serde_json::from_slice(&envelope.payload)
            .map_err(|_| Status::invalid_argument("invalid motion request"))?;
        let evidence = self.dispatch(&token, &envelope, &request).await.map_err(|error| {
            tracing::warn!(request_id=%request.request_id, error=%error, "motion request refused or observation unavailable");
            Status::failed_precondition("motion request refused or observation unavailable")
        })?;
        let reply = MotionReply {
            version: MOTION_CONTRACT_VERSION,
            request_id: request.request_id,
            device_id: self.device_id.clone(),
            evidence,
        };
        let payload = serde_json::to_vec(&reply)
            .map_err(|_| Status::internal("motion receipt serialization failed"))?;
        if payload.len() > MAX_MOTION_PAYLOAD_BYTES {
            return Err(Status::internal("motion receipt exceeds protocol bounds"));
        }
        let signature = sign_payload(&self.signing_key, &motion_reply_payload(&payload));
        Ok(Response::new(SignedMotionPayload { payload, signature }))
    }
}

fn validate_reason(reason: &str) -> Result<()> {
    ensure!(
        !reason.trim().is_empty() && reason.len() <= 256,
        "invalid motion control reason"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
