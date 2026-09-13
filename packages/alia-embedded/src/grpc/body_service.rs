//! Signed, bounded control requests. Native work belongs to MotionRuntime;
//! cancelling an RPC only stops observation, never drops its device owner.
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use alia_core::body::*;
use alia_proto::body::{SignedBodyPayload, body_control_server::BodyControl};
use alia_transport::{body_reply_payload, sign_payload};
use anyhow::{Result, ensure};
use ed25519_dalek::SigningKey;
use tonic::{Request, Response, Status};

use super::auth::extract_bearer_token;
use crate::{
    binding::BindingManager,
    module::{
        motion::{BodyAuthorization, MotionRuntime},
        playback_session::PlaybackSession,
    },
};

pub struct EmbeddedBodyService {
    binding: Arc<BindingManager>,
    motion: Arc<MotionRuntime>,
    playback: Arc<PlaybackSession>,
    device_id: String,
    signing_key: SigningKey,
}

impl EmbeddedBodyService {
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

    fn validate_request(&self, request: &BodyRequest) -> Result<()> {
        ensure!(
            request.version == BODY_CONTRACT_VERSION,
            "unsupported body protocol"
        );
        ensure!(
            !request.request_id.is_nil() && request.device_id == self.device_id,
            "body request target or nonce mismatch"
        );
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        ensure!(
            now.abs_diff(u128::from(request.issued_at_unix_ms)) <= 30_000,
            "body request timestamp expired"
        );
        match &request.operation {
            BodyOperation::Inspect => {}
            BodyOperation::Open {
                device_boot_id,
                server_instance_id,
                ..
            } => ensure!(
                !device_boot_id.is_nil() && !server_instance_id.is_nil(),
                "invalid body control identity"
            ),
            BodyOperation::Submit { command } => command.validate()?,
            BodyOperation::Observe { identity, .. } => {
                ensure!(identity.is_valid(), "invalid body observation identity")
            }
            BodyOperation::Cancel { command, reason } => {
                command.validate()?;
                validate_reason(reason)?;
            }
            BodyOperation::Close { lease, reason } => {
                ensure!(lease.is_valid(), "invalid body close identity");
                validate_reason(reason)?;
            }
        }
        Ok(())
    }

    async fn authenticated<T>(
        &self,
        token: &str,
        envelope: &SignedBodyPayload,
        request: &BodyRequest,
        admit: impl FnOnce(Instant) -> Result<T>,
    ) -> Result<T> {
        self.binding
            .with_verified_body(
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

    fn renew_if_current(&self, identity: &BodyCommandIdentity, expires_at: Instant) -> Result<()> {
        if self.motion.control_state().lease.as_ref() == Some(&identity.lease) {
            self.motion
                .renew_control(&identity.lease, BodyAuthorization { expires_at })?;
        }
        Ok(())
    }

    async fn dispatch(
        &self,
        token: &str,
        envelope: &SignedBodyPayload,
        request: &BodyRequest,
    ) -> Result<BodyEvidence> {
        match &request.operation {
            BodyOperation::Inspect => {
                self.authenticated(token, envelope, request, |_| {
                    Ok(BodyEvidence::Control {
                        state: self.motion.control_state(),
                    })
                })
                .await
            }
            BodyOperation::Open {
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
                        BodyAuthorization { expires_at },
                    )?;
                    Ok(BodyEvidence::Control {
                        state: BodyControlState {
                            device_boot_id: lease.device_boot_id,
                            generation: lease.generation,
                            lease: Some(lease),
                            available: before.available,
                        },
                    })
                })
                .await
            }
            BodyOperation::Submit { command } => {
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
                .map_err(|_| anyhow::anyhow!("body preparation confirmation timed out"))
                .and_then(|result| result);
                let admission = match preparation {
                    Ok(prepared) => {
                        self.authenticated(token, envelope, request, |expires_at| {
                            let submit = || {
                                self.motion
                                    .submit(prepared, BodyAuthorization { expires_at })
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
                        tracing::debug!(request_id=%request.request_id, error=%error, "body admission failed; checking exact rejection evidence");
                        // The same Motion gate either returns existing facts or
                        // retires a provably unaccepted sequence. A cache miss
                        // from an old lease cannot be turned into a zero-write
                        // claim. Late preparation can never revive this identity.
                        self.authenticated(token, envelope, request, |_| {
                            self.motion.reject(
                                command.clone(),
                                canonical_command,
                                "body command was refused before native admission",
                            )
                        })
                        .await?
                    }
                };
                Ok(BodyEvidence::Progress {
                    progress: observation.snapshot(),
                })
            }
            BodyOperation::Observe {
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
                    "body observation cursor exceeds device evidence"
                );
                if snapshot.receipt_sequence == *after_receipt_sequence && snapshot.end.is_none() {
                    let _ = tokio::time::timeout(Duration::from_millis(750), observation.changed())
                        .await;
                }
                // Revocation may have happened while waiting; the original
                // command facts remain queryable, without opening a new writer.
                Ok(BodyEvidence::Progress {
                    progress: observation.snapshot(),
                })
            }
            BodyOperation::Cancel { command, reason } => {
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
                    "body cancellation has no exact quiescent checkpoint"
                );
                Ok(BodyEvidence::Progress { progress })
            }
            BodyOperation::Close { lease, reason } => {
                self.authenticated(token, envelope, request, |_| {
                    self.motion.close_control(lease, reason)?;
                    // Closing admission is not evidence that an in-flight native
                    // write or physical motor stopped. Query each known command.
                    Ok(BodyEvidence::Control {
                        state: self.motion.control_state(),
                    })
                })
                .await
            }
        }
    }
}

#[tonic::async_trait]
impl BodyControl for EmbeddedBodyService {
    async fn exchange(
        &self,
        request: Request<SignedBodyPayload>,
    ) -> std::result::Result<Response<SignedBodyPayload>, Status> {
        let token = extract_bearer_token(request.metadata())?;
        let envelope = request.into_inner();
        if envelope.payload.len() > MAX_BODY_PAYLOAD_BYTES || envelope.signature.len() != 64 {
            return Err(Status::invalid_argument(
                "body payload or signature exceeds protocol bounds",
            ));
        }
        let request: BodyRequest = serde_json::from_slice(&envelope.payload)
            .map_err(|_| Status::invalid_argument("invalid body request"))?;
        let evidence = self.dispatch(&token, &envelope, &request).await.map_err(|error| {
            tracing::warn!(request_id=%request.request_id, error=%error, "body request refused or observation unavailable");
            Status::failed_precondition("body request refused or observation unavailable")
        })?;
        let reply = BodyReply {
            version: BODY_CONTRACT_VERSION,
            request_id: request.request_id,
            device_id: self.device_id.clone(),
            evidence,
        };
        let payload = serde_json::to_vec(&reply)
            .map_err(|_| Status::internal("body receipt serialization failed"))?;
        if payload.len() > MAX_BODY_PAYLOAD_BYTES {
            return Err(Status::internal("body receipt exceeds protocol bounds"));
        }
        let signature = sign_payload(&self.signing_key, &body_reply_payload(&payload));
        Ok(Response::new(SignedBodyPayload { payload, signature }))
    }
}

fn validate_reason(reason: &str) -> Result<()> {
    ensure!(
        !reason.trim().is_empty() && reason.len() <= 256,
        "invalid body control reason"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
