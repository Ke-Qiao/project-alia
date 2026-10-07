//! Original output ownership and control transport faults against real PostgreSQL.
//! Fake device barriers control ACK timing; all admissions and facts use the
//! production DeliveryService and repository.
mod delivery_support;

use alia_core::{
    delivery::{
        ObservationEnd, PlaybackIdentity, PlaybackProgress, SubmissionBackend, SubmissionBoundary,
    },
    output_control::{
        BoundControlCommand, BoundControlReceipt, ControlMode, OutputBinding, OutputRegistration,
    },
    playback_control::{
        CommandOutcome, ControlAction, ControlPhase, ControlReceipt, ControlSnapshot, HoldReason,
        ResumeAuthorization, ResumeKind,
    },
};
use alia_server::{
    channels::adapter::*,
    modules::{
        delivery::{
            AudioDelivery, DeliveryService, OutputProducer,
            repository::control::{ControlCommandStatus, OutputControlStatus},
            types::*,
        },
        platform::lifecycle::ModuleLifecycle,
    },
    providers::tts::audio_budget::ResponseAudioBudget,
};
use async_trait::async_trait;
use delivery_support::*;
use futures::{FutureExt, StreamExt};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::json;
use std::{
    collections::HashMap,
    future::Future,
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn sql(query: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, query, values)
}

struct InFlight<'a> {
    dropped: &'a AtomicUsize,
    finished: bool,
}
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
}

struct Device {
    owner: Uuid,
    db: DatabaseConnection,
    backend: SubmissionBackend,
    scope: OnceLock<alia_core::delivery::VoiceScope>,
    registration: OnceLock<OutputRegistration>,
    block_register: AtomicBool,
    register_entered: Semaphore,
    register_release: Semaphore,
    register_dropped: AtomicUsize,
    commands: Mutex<Vec<BoundControlCommand>>,
    observed_commands: Mutex<Vec<BoundControlCommand>>,
    receipts: Mutex<HashMap<Uuid, BoundControlReceipt>>,
    observation: AtomicU64,
    lose_ack: AtomicBool,
    deny_observe: AtomicBool,
    block_resume: AtomicBool,
    resume_entered: Semaphore,
    resume_release: Semaphore,
    resume_dropped: AtomicUsize,
    block_stop: AtomicBool,
    stop_entered: Semaphore,
    stop_release: Semaphore,
    stop_finished: AtomicUsize,
    stop_dropped: AtomicUsize,
    reservations: AtomicUsize,
    media_calls: AtomicUsize,
    media_entered: Semaphore,
    media_release: Semaphore,
    media_progress: Mutex<Option<PlaybackProgress>>,
}
impl Device {
    fn new(f: &Fixture) -> Self {
        Self {
            owner: f.user,
            db: f.db.clone(),
            backend: SubmissionBackend {
                instance_id: Uuid::new_v4(),
                boundary: SubmissionBoundary::AlsaWriteAccepted,
                output_sample_rate_hz: 48000,
                output_channels: 2,
            },
            scope: OnceLock::new(),
            registration: OnceLock::new(),
            block_register: AtomicBool::new(false),
            register_entered: Semaphore::new(0),
            register_release: Semaphore::new(0),
            register_dropped: AtomicUsize::new(0),
            commands: Mutex::default(),
            observed_commands: Mutex::default(),
            receipts: Mutex::default(),
            observation: AtomicU64::new(0),
            lose_ack: AtomicBool::new(false),
            deny_observe: AtomicBool::new(false),
            block_resume: AtomicBool::new(false),
            resume_entered: Semaphore::new(0),
            resume_release: Semaphore::new(0),
            resume_dropped: AtomicUsize::new(0),
            block_stop: AtomicBool::new(false),
            stop_entered: Semaphore::new(0),
            stop_release: Semaphore::new(0),
            stop_finished: AtomicUsize::new(0),
            stop_dropped: AtomicUsize::new(0),
            reservations: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            media_entered: Semaphore::new(0),
            media_release: Semaphore::new(0),
            media_progress: Mutex::default(),
        }
    }
    fn receipt(&self, command: &BoundControlCommand) -> BoundControlReceipt {
        let held = !matches!(command.command.action, ControlAction::Resume { .. });
        let reason = match command.command.action {
            ControlAction::Pause { reason, .. } => reason,
            _ => HoldReason::ExplicitWait,
        };
        BoundControlReceipt {
            command: command.clone(),
            intent_digest: command.intent_digest(),
            observation_sequence: self.observation.fetch_add(1, Ordering::SeqCst) + 1,
            backend: self.backend.clone(),
            mode: ControlMode::SoftwareGate,
            native_writes_quiescent: true,
            progress: vec![],
            receipt: ControlReceipt {
                command_id: command.command.command_id,
                outcome: CommandOutcome::Applied,
                snapshot: ControlSnapshot {
                    scope: command.binding.scope,
                    phase: if held {
                        ControlPhase::Held
                    } else {
                        ControlPhase::Running
                    },
                    hold_id: held.then_some(command.command.hold_id),
                    reason: held.then_some(reason),
                    sequence: command.command.sequence,
                    remaining_ms: held.then_some(30000),
                    retired: None,
                    local_capture_hold: None,
                },
            },
        }
    }
    fn release_all(&self) {
        self.register_release.add_permits(32);
        self.resume_release.add_permits(32);
        self.stop_release.add_permits(32);
        self.media_release.add_permits(32);
    }
}
#[async_trait]
impl ModuleLifecycle for Device {
    fn name(&self) -> &'static str {
        "output-lifecycle-test"
    }
}
impl Device {
    async fn retire_model_scope(&self, target: &ChannelTarget, _: &str) -> anyhow::Result<()> {
        assert_eq!(target.user_id, self.owner.to_string());
        assert_eq!(target.voice, self.scope.get().copied());
        let mut owner = InFlight {
            dropped: &self.stop_dropped,
            finished: false,
        };
        self.stop_entered.add_permits(1);
        if self.block_stop.load(Ordering::SeqCst) {
            self.stop_release.acquire().await?.forget();
        }
        self.stop_finished.fetch_add(1, Ordering::SeqCst);
        owner.finished = true;
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for Device {
    fn channel_type(&self) -> &'static str {
        "test"
    }
    fn channel_id(&self) -> &str {
        "test-device"
    }
    fn owner_user_id(&self) -> Option<Uuid> {
        Some(self.owner)
    }
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            voice_output: true,
            audio_streaming: true,
            ..Default::default()
        }
    }

    async fn register_output(&self, binding: OutputBinding) -> anyhow::Result<OutputRegistration> {
        let registration = OutputRegistration {
            binding,
            backend: self.backend.clone(),
            maximum_hold_ms: 60000,
            initiative_capture_gate_version: 0,
        };
        self.registration
            .set(registration.clone())
            .map_err(|_| anyhow::anyhow!("duplicate device registration"))?;
        if self.block_register.load(Ordering::SeqCst) {
            let mut owner = InFlight {
                dropped: &self.register_dropped,
                finished: false,
            };
            self.register_entered.add_permits(1);
            self.register_release.acquire().await?.forget();
            owner.finished = true;
        }
        Ok(registration)
    }
    async fn control_output(
        &self,
        command: &BoundControlCommand,
    ) -> anyhow::Result<BoundControlReceipt> {
        let row = self
            .db
            .query_one(sql(
                "SELECT command,intent_digest FROM output_control_commands WHERE command_id=$1",
                vec![command.command.command_id.into()],
            ))
            .await?
            .expect("Control RPC preceded durable admission");
        assert_eq!(
            row.try_get::<serde_json::Value>("", "command")?,
            json!(command)
        );
        assert_eq!(
            row.try_get::<String>("", "intent_digest")?,
            command.intent_digest()
        );
        self.commands.lock().unwrap().push(command.clone());
        if self.block_resume.load(Ordering::SeqCst)
            && matches!(command.command.action, ControlAction::Resume { .. })
        {
            let mut owner = InFlight {
                dropped: &self.resume_dropped,
                finished: false,
            };
            self.resume_entered.add_permits(1);
            self.resume_release.acquire().await?.forget();
            owner.finished = true;
        }
        let receipt = self.receipt(command);
        receipt.validate(command)?;
        self.receipts
            .lock()
            .unwrap()
            .insert(command.command.command_id, receipt.clone());
        if self.lose_ack.swap(false, Ordering::SeqCst) {
            anyhow::bail!("injected lost Control ACK after device application");
        }
        Ok(receipt)
    }
    async fn observe_output_control(
        &self,
        command: &BoundControlCommand,
    ) -> anyhow::Result<BoundControlReceipt> {
        self.observed_commands.lock().unwrap().push(command.clone());
        if self.deny_observe.load(Ordering::SeqCst) {
            anyhow::bail!("original observation unavailable");
        }
        let mut receipt = self
            .receipts
            .lock()
            .unwrap()
            .get(&command.command.command_id)
            .cloned()
            .expect("Observe used a new command identity");
        assert_eq!(&receipt.command, command);
        receipt.observation_sequence = self.observation.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(receipt)
    }

    async fn retire_voice_output(
        &self,
        request: &alia_core::output_control::OutputRetirementRequest,
    ) -> anyhow::Result<alia_core::output_control::OutputRetirementReceipt> {
        use alia_core::output_control::{OutputRetirementEvidence, OutputRetirementReceipt};
        request.validate()?;
        assert_eq!(Some(request.scope), self.scope.get().copied());
        if let Some(registration) = self.registration.get() {
            assert_eq!(request.binding, Some(registration.binding));
        }
        if let Some(expected) = &request.expected_backend {
            assert_eq!(expected, &self.backend);
        }
        self.retire_model_scope(
            &ChannelTarget {
                user_id: self.owner.to_string(),
                voice: Some(request.scope),
            },
            "original lifecycle model retired",
        )
        .await?;
        // This fake has an explicit scope gate/stop barrier but no native
        // write queue. Do not invent new media checkpoints from that barrier.
        let evidence = if self.registration.get().is_some() {
            OutputRetirementEvidence::Settled {
                backend: self.backend.clone(),
                progress: vec![],
            }
        } else {
            assert_eq!(self.media_calls.load(Ordering::SeqCst), 0);
            OutputRetirementEvidence::NoNativeOwner
        };
        let receipt = OutputRetirementReceipt {
            scope: request.scope,
            binding: request.binding,
            evidence,
        };
        receipt.validate(request)?;
        Ok(receipt)
    }
    fn reserve_playback(
        &self,
        target: &ChannelTarget,
        playback_id: Uuid,
    ) -> anyhow::Result<PlaybackIdentity> {
        let sequence = self.reservations.fetch_add(1, Ordering::SeqCst) as u64 + 1;
        Ok(PlaybackIdentity {
            scope: target.voice.expect("scoped playback required"),
            playback_id,
            playback_sequence: sequence,
        })
    }
    async fn deliver_audio(
        &self,
        _: &ChannelTarget,
        identity: PlaybackIdentity,
        mut audio: AudioPayloadStream,
        observer: Arc<dyn PlaybackObserver>,
        cancel: CancellationToken,
    ) -> anyhow::Result<AudioDeliveryReport> {
        self.media_calls.fetch_add(1, Ordering::SeqCst);
        observer.start(24000, 1).await?;
        let mut frames = 0;
        while let Some(payload) = audio.next().await {
            frames += payload?.data.len() as u64 / 2;
        }
        observer.offered(frames).await?;
        let mut progress = receipt(identity, 1, frames);
        *self.media_progress.lock().unwrap() = Some(progress.clone());
        observer.progress(progress.clone()).await?;
        self.media_entered.add_permits(1);
        tokio::select! { biased;
            _ = cancel.cancelled() => return Ok(AudioDeliveryReport::unobserved("cancelled media owner")),
            permit = self.media_release.acquire() => permit?.forget(),
        }
        progress.receipt_sequence += 1;
        progress.end_accepted = true;
        progress.submitted_source_frames = Some(frames);
        progress.backend = Some(self.backend.clone());
        progress.observation_end = Some(ObservationEnd::SubmissionFinished);
        *self.media_progress.lock().unwrap() = Some(progress.clone());
        observer.progress(progress.clone()).await?;
        Ok(AudioDeliveryReport {
            progress: Some(progress),
            observation_end: ObservationEnd::SubmissionFinished,
        })
    }
    async fn cancel_playback(
        &self,
        identity: PlaybackIdentity,
        reason: &str,
    ) -> anyhow::Result<PlaybackProgress> {
        let mut progress = self
            .media_progress
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no device progress"))?;
        assert_eq!(progress.identity, identity);
        if progress.observation_end.is_none() {
            progress.receipt_sequence += 1;
            progress.observation_end = Some(ObservationEnd::Cancelled {
                reason: reason.into(),
            });
        }
        Ok(progress)
    }
}

#[derive(Clone)]
struct Scene {
    f: Fixture,
    service: Arc<DeliveryService>,
    device: Arc<Device>,
    expected_shutdown_failure: Arc<AtomicBool>,
}
impl Scene {
    async fn output(&self) -> (Arc<OutputProducer>, ResponseRecord) {
        let (producer, response) = self.unbound_output().await;
        producer.bind(response.clone()).await.unwrap();
        (producer, response)
    }
    async fn unbound_output(&self) -> (Arc<OutputProducer>, ResponseRecord) {
        let scope = identity().scope;
        self.device.scope.set(scope).unwrap();
        let mut input = self.f.response().await;
        input.content["voice_scope"] = json!(scope);
        let producer = self
            .service
            .begin_voice_output(
                self.f.user,
                input.key.request_id,
                scope,
                self.device.clone(),
                CancellationToken::new(),
                None,
            )
            .unwrap();
        let response = self.service.prepare(input).await.unwrap();
        (producer, response)
    }
    async fn audio(&self, response: &ResponseRecord) -> AudioDelivery {
        let data = ResponseAudioBudget::for_response(response.response_id)
            .unwrap()
            .copy_from_slice(&[0; 8])
            .await
            .unwrap()
            .into_buffer();
        AudioDelivery {
            response_id: response.response_id,
            output_key: "speech:0".into(),
            content: json!({"text":"hello"}),
            channel: self.device.clone(),
            target: ChannelTarget {
                user_id: self.f.user.to_string(),
                voice: Some(self.device.registration.get().unwrap().binding.scope),
            },
            audio: Box::pin(futures::stream::iter([Ok(AudioPayload {
                data,
                sample_rate: 24000,
                channels: 1,
                format: AudioFormat::Pcm16,
            })])),
            cancel: CancellationToken::new(),
        }
    }
    async fn wait_receipted(&self, count: usize) {
        eventually(|| async {
            let binding = &self.device.registration.get().unwrap().binding;
            let view = self
                .f
                .repo()
                .query_control(self.f.user, binding)
                .await
                .unwrap();
            view.commands.len() == count
                && view
                    .commands
                    .iter()
                    .all(|command| command.status == ControlCommandStatus::Receipted)
        })
        .await;
    }
    async fn wait_retired(&self) {
        eventually(|| async {
            self.f
                .repo()
                .query_control(
                    self.f.user,
                    &self.device.registration.get().unwrap().binding,
                )
                .await
                .unwrap()
                .scope
                .status
                == OutputControlStatus::Retired
        })
        .await;
    }
}
async fn with_scene<F, Fut>(test: F)
where
    F: FnOnce(Scene) -> Fut,
    Fut: Future<Output = ()>,
{
    with_db(|f| async move {
        let service = DeliveryService::new(f.repo(), &CancellationToken::new());
        service.start().await.unwrap();
        let scene = Scene {
            device: Arc::new(Device::new(&f)),
            f,
            service,
            expected_shutdown_failure: Arc::new(AtomicBool::new(false)),
        };
        // Leave ten seconds of the outer fixture budget for releasing fake
        // barriers and joining production owners before the schema is dropped.
        let result = AssertUnwindSafe(tokio::time::timeout(
            Duration::from_secs(34),
            test(scene.clone()),
        ))
        .catch_unwind()
        .await;
        scene.device.release_all();
        let shutdown = scene.service.shutdown().await;
        assert!(
            !scene.service.shutdown_pending(),
            "cleanup failed to join output/media owners: {shutdown:?}"
        );
        match result {
            Err(error) => std::panic::resume_unwind(error),
            Ok(Err(_)) => panic!("output lifecycle body exceeded 34 seconds"),
            Ok(Ok(())) => {}
        }
        if scene.expected_shutdown_failure.load(Ordering::SeqCst) {
            assert!(shutdown.is_err());
        } else {
            shutdown.unwrap();
        }
    })
    .await;
}
async fn acquire(semaphore: &Semaphore) {
    tokio::time::timeout(Duration::from_secs(3), semaphore.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}
async fn eventually<F, Fut>(mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition().await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected owned operation did not finish");
}
fn authorization() -> ResumeAuthorization {
    ResumeAuthorization {
        kind: ResumeKind::ExplicitContinue,
        detection_id: Uuid::new_v4(),
        through_sequence: 10,
    }
}

#[tokio::test]
async fn forged_bind_candidate_cannot_cancel_another_request_response() {
    with_scene(|s| async move {
        let (producer, original) = s.unbound_output().await;
        let foreign = s.service.prepare(s.f.response().await).await.unwrap();
        assert_ne!(foreign.key.request_id, original.key.request_id);
        let mut forged = foreign.clone();
        forged.key.request_id = original.key.request_id;
        // A caller-controlled record must not become cleanup authority merely
        // by matching the producer's request fields before actor validation.
        let mut binding = Box::pin(producer.bind(forged));
        assert!(futures::poll!(binding.as_mut()).is_pending());
        producer.output().retire();
        assert!(binding.await.is_err());
        acquire(&s.device.stop_entered).await;
        s.expected_shutdown_failure.store(true, Ordering::SeqCst);
        let failure = s.service.shutdown().await.unwrap_err().to_string();
        assert!(
            failure.contains("output cleanup candidate differs from canonical Response"),
            "{failure}"
        );
        assert!(
            !s.service.shutdown_pending(),
            "identity rejection did not join its output owner"
        );
        let stored =
            s.f.repo()
                .response(s.f.user, foreign.response_id)
                .await
                .unwrap();
        assert_eq!(
            json!(stored),
            json!(foreign),
            "unverified Bind record changed another request's Response"
        );
        assert!(s.device.registration.get().is_none());
        assert_eq!(s.device.reservations.load(Ordering::SeqCst), 0);
        assert_eq!(s.device.media_calls.load(Ordering::SeqCst), 0);
        assert_eq!(s.f.count("delivery_responses").await, 2);
        assert_eq!(s.f.count("output_control_scopes").await, 0);
    })
    .await;
}

#[tokio::test]
async fn queued_bind_then_immediate_retirement_cancels_original_response() {
    with_scene(|s| async move {
        let (producer, response) = s.unbound_output().await;
        let output = producer.output();
        let mut binding = Box::pin(producer.bind(response.clone()));
        // No yield separates Bind enqueue from retirement. The control actor
        // cannot have consumed the queued canonical Response on this executor.
        assert!(futures::poll!(binding.as_mut()).is_pending());
        output.retire();
        assert!(binding.await.is_err());
        acquire(&s.device.stop_entered).await;
        eventually(|| async {
            s.f.repo()
                .response(s.f.user, response.response_id)
                .await
                .unwrap()
                .cancel_requested
        })
        .await;
        assert!(
            s.device.registration.get().is_none(),
            "retired queued Bind reached device registration"
        );
        assert_eq!(s.device.stop_finished.load(Ordering::SeqCst), 1);
        assert_eq!(s.device.reservations.load(Ordering::SeqCst), 0);
        assert_eq!(s.device.media_calls.load(Ordering::SeqCst), 0);
        assert_eq!(s.f.count("delivery_responses").await, 1);
        assert_eq!(s.f.count("output_control_scopes").await, 0);
    })
    .await;
}

#[tokio::test]
async fn cancellation_during_register_ack_wait_cancels_original_response() {
    interrupted_registration(BindInterruption::Retire).await;
}

#[tokio::test]
async fn semantic_expiry_during_register_ack_wait_cancels_original_response() {
    interrupted_registration(BindInterruption::SemanticExpiry).await;
}

#[tokio::test]
async fn response_cancellation_during_register_ack_wait_stops_original_bind() {
    interrupted_registration(BindInterruption::CancelResponse).await;
}

enum BindInterruption {
    Retire,
    SemanticExpiry,
    CancelResponse,
}

async fn interrupted_registration(interruption: BindInterruption) {
    with_scene(|s| async move {
        s.device.block_register.store(true, Ordering::SeqCst);
        let (producer, response) = s.unbound_output().await;
        let output = producer.output();
        let mut binding = Box::pin(producer.bind(response.clone()));
        assert!(futures::poll!(binding.as_mut()).is_pending());
        acquire(&s.device.register_entered).await;
        assert!(
            !s.f.repo()
                .response(s.f.user, response.response_id)
                .await
                .unwrap()
                .cancel_requested
        );
        match interruption {
            BindInterruption::Retire => output.retire(),
            BindInterruption::SemanticExpiry => {
                assert!(output.near_end_speech(10).unwrap());
            }
            BindInterruption::CancelResponse => {
                assert!(
                    s.service
                        .cancel_response(s.f.user, response.response_id)
                        .await
                        .unwrap()
                        .cancel_requested
                );
            }
        }
        tokio::time::timeout(Duration::from_secs(5), s.device.stop_entered.acquire())
            .await
            .expect("original registration was not fenced after cancellation/expiry")
            .unwrap()
            .forget();
        assert!(binding.await.is_err(), "unacknowledged Bind succeeded");
        assert!(!output.is_alive());
        assert!(producer.wait_open().await.is_err());
        eventually(|| async {
            s.f.repo()
                .response(s.f.user, response.response_id)
                .await
                .unwrap()
                .cancel_requested
        })
        .await;
        assert_eq!(s.device.register_dropped.load(Ordering::SeqCst), 1);
        assert_eq!(s.device.stop_finished.load(Ordering::SeqCst), 1);
        assert_eq!(s.device.reservations.load(Ordering::SeqCst), 0);
        assert_eq!(s.device.media_calls.load(Ordering::SeqCst), 0);
        assert!(s.device.commands.lock().unwrap().is_empty());
        assert_eq!(
            s.f.count("delivery_responses").await,
            1,
            "cancelled Bind created another Response"
        );
        assert_eq!(
            s.f.count("output_control_scopes").await,
            0,
            "unreceived registration ACK became a durable device fact"
        );
    })
    .await;
}

#[tokio::test]
async fn enqueue_owns_media_before_waiter_and_last_producer_drop() {
    with_scene(|s| async move {
        let (producer, response) = s.output().await;
        let output = producer.output();
        let mut waiter = Box::pin(s.service.deliver_audio(s.audio(&response).await));
        // No await/yield between enqueue and dropping both caller-owned handles:
        // on this current-thread executor the worker cannot have retained it yet.
        assert!(futures::poll!(waiter.as_mut()).is_pending());
        drop(waiter);
        drop(producer);
        assert!(
            output.is_alive(),
            "queued original media lost its output owner"
        );
        acquire(&s.device.media_entered).await;
        assert!(output.is_alive());
        s.device.media_release.add_permits(1);
        s.wait_retired().await;
        let first = s
            .service
            .list_deliveries(s.f.user, response.response_id)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(first.status, DeliveryStatus::Submitted);
        assert_eq!(first.submitted_frames, Some(4));
        assert_eq!(first.presented_frames, None);
        // A caller retry after live ownership ended only reads the original fact.
        let duplicate = s
            .service
            .deliver_audio(s.audio(&response).await)
            .await
            .unwrap();
        assert_eq!(duplicate.delivery_id, first.delivery_id);
        assert_eq!(s.device.media_calls.load(Ordering::SeqCst), 1);
        assert_eq!(s.device.reservations.load(Ordering::SeqCst), 1);
        assert_eq!(s.device.stop_finished.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn lost_control_ack_observes_same_intent_without_reapplying() {
    with_scene(|s| async move {
        let (producer, _) = s.output().await;
        let output = producer.output();
        s.device.lose_ack.store(true, Ordering::SeqCst);
        assert!(output.explicit_wait(Uuid::new_v4(), 10).unwrap());
        s.wait_receipted(1).await;
        assert!(output.is_held());
        let applied = s.device.commands.lock().unwrap().clone();
        assert_eq!(applied.len(), 1);
        assert_eq!(
            s.device.observed_commands.lock().unwrap().as_slice(),
            applied.as_slice()
        );
        let view =
            s.f.repo()
                .query_control(s.f.user, &applied[0].binding)
                .await
                .unwrap();
        assert_eq!(view.commands[0].command, applied[0]);
        assert_eq!(
            view.commands[0]
                .first_receipt
                .as_ref()
                .unwrap()
                .receipt
                .outcome,
            CommandOutcome::Applied
        );
        assert!(output.request_resume(authorization()).unwrap());
        producer.wait_open().await.unwrap();
        s.wait_receipted(2).await;
        assert_eq!(s.device.commands.lock().unwrap().len(), 2);
        assert_eq!(s.device.observed_commands.lock().unwrap().len(), 1);
    })
    .await;
}

#[tokio::test]
async fn unavailable_ack_and_observation_retire_unknown_without_replay() {
    with_scene(|s| async move {
        let (producer, _) = s.output().await;
        let output = producer.output();
        s.device.lose_ack.store(true, Ordering::SeqCst);
        s.device.deny_observe.store(true, Ordering::SeqCst);
        assert!(output.explicit_wait(Uuid::new_v4(), 10).unwrap());
        s.wait_retired().await;
        assert!(!output.is_alive());
        assert!(producer.wait_open().await.is_err());
        let view =
            s.f.repo()
                .query_control(s.f.user, &s.device.registration.get().unwrap().binding)
                .await
                .unwrap();
        assert_eq!(view.commands.len(), 1);
        assert_eq!(view.commands[0].status, ControlCommandStatus::Unknown);
        assert!(view.commands[0].first_receipt.is_none());
        assert_eq!(s.device.commands.lock().unwrap().len(), 1);
        assert_eq!(s.device.observed_commands.lock().unwrap().len(), 1);
        assert_eq!(s.device.stop_finished.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn later_audio_permit_deadline_fences_inflight_resume_without_opening_gate() {
    with_scene(|s| async move {
        let (producer, response) = s.output().await;
        let output = producer.output();
        assert!(output.explicit_wait(Uuid::new_v4(), 10).unwrap());
        s.wait_receipted(1).await;
        let registration = s.device.registration.get().unwrap();
        let audio = s.f.repo().prepare_delivery(s.f.user, NewDelivery { response_id: response.response_id, output_key: "independent-original-audio".into(), content: json!({"text":"earlier part"}), identity: Some(PlaybackIdentity { scope: registration.binding.scope, ..identity() }), owner_id: registration.binding.server_instance_id }).await.unwrap();
        s.f.repo().dispatch(s.f.user, audio.delivery_id, registration.binding.server_instance_id).await.unwrap().unwrap();
        let deadline: chrono::DateTime<chrono::Utc> = s.f.db.query_one(sql("UPDATE deliveries SET observation_deadline_at=clock_timestamp()+interval '1 second' WHERE delivery_id=$1 RETURNING observation_deadline_at", vec![audio.delivery_id.into()])).await.unwrap().unwrap().try_get("", "observation_deadline_at").unwrap();
        s.device.block_resume.store(true, Ordering::SeqCst);
        assert!(output.request_resume(authorization()).unwrap());
        acquire(&s.device.resume_entered).await;
        let admitted = s.f.repo().query_control(s.f.user, &registration.binding).await.unwrap();
        assert_eq!(admitted.commands[1].deadline_at, deadline);
        assert!(output.is_held());
        // The fake peer could eventually say Applied; it cannot extend the
        // original SQL permit. Expiry must fence without waiting for that ACK.
        acquire(&s.device.stop_entered).await;
        s.wait_retired().await;
        assert!(!output.is_alive());
        assert!(producer.wait_open().await.is_err());
        assert_eq!(s.device.resume_dropped.load(Ordering::SeqCst), 1);
        let view = s.f.repo().query_control(s.f.user, &registration.binding).await.unwrap();
        assert_eq!(view.commands[1].status, ControlCommandStatus::Unknown);
        assert!(view.commands[1].first_receipt.is_none(), "unreceived ACK became a fabricated outcome");
        assert_eq!(s.device.commands.lock().unwrap().len(), 2);
        assert!(s.device.observed_commands.lock().unwrap().is_empty());
        assert!(s.f.repo().response(s.f.user, response.response_id).await.unwrap().cancel_requested);
    }).await;
}

#[tokio::test]
async fn scope_fence_timeout_retains_original_future_until_real_join() {
    with_scene(|s| async move {
        let (producer, response) = s.output().await;
        s.device.block_stop.store(true, Ordering::SeqCst);
        s.expected_shutdown_failure.store(true, Ordering::SeqCst);
        producer.output().retire();
        drop(producer);
        acquire(&s.device.stop_entered).await;
        // The local cancellation is known even though the device's final stop
        // remains unknown. Persist it without dropping its original fence.
        s.wait_retired().await;
        assert!(
            s.f.repo()
                .response(s.f.user, response.response_id)
                .await
                .unwrap()
                .cancel_requested
        );
        assert_eq!(s.device.stop_finished.load(Ordering::SeqCst), 0);
        assert_eq!(s.device.stop_dropped.load(Ordering::SeqCst), 0);
        let mut shutdown = Box::pin(s.service.shutdown());
        assert!(
            tokio::time::timeout(Duration::from_millis(2150), shutdown.as_mut())
                .await
                .is_err(),
            "shutdown completed while its original native fence still ran"
        );
        assert_eq!(s.device.stop_dropped.load(Ordering::SeqCst), 0);
        assert_eq!(s.device.stop_finished.load(Ordering::SeqCst), 0);
        assert!(s.service.shutdown_pending());
        s.device.stop_release.add_permits(1);
        let failure = shutdown.await.unwrap_err().to_string();
        assert!(failure.contains("output fence deadline"), "{failure}");
        assert_eq!(s.device.stop_finished.load(Ordering::SeqCst), 1);
        assert_eq!(s.device.stop_dropped.load(Ordering::SeqCst), 0);
        assert!(!s.service.shutdown_pending());
        s.wait_retired().await;
    })
    .await;
}

#[tokio::test]
async fn held_original_media_keeps_sql_lease_heartbeat_alive() {
    with_scene(|s| async move {
        let (producer, response) = s.output().await;
        let output = producer.output();
        let mut waiter = Box::pin(s.service.deliver_audio(s.audio(&response).await));
        assert!(futures::poll!(waiter.as_mut()).is_pending());
        acquire(&s.device.media_entered).await;
        let delivery = s.service.list_deliveries(s.f.user, response.response_id).await.unwrap().remove(0);
        assert!(output.explicit_wait(Uuid::new_v4(), 10).unwrap());
        s.wait_receipted(1).await;
        let shortened: chrono::DateTime<chrono::Utc> = s.f.db.query_one(sql("UPDATE deliveries SET lease_expires_at=clock_timestamp()+interval '10 seconds' WHERE delivery_id=$1 RETURNING lease_expires_at", vec![delivery.delivery_id.into()])).await.unwrap().unwrap().try_get("", "lease_expires_at").unwrap();
        tokio::time::timeout(Duration::from_secs(7), async {
            loop {
                let current = s.f.repo().get(s.f.user, delivery.delivery_id).await.unwrap();
                assert!(!current.status.is_terminal());
                if current.lease_expires_at > shortened { break; }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }).await.expect("Held suppressed the original media owner's SQL heartbeat");
        assert!(output.is_held());
        assert!(output.request_resume(authorization()).unwrap());
        producer.wait_open().await.unwrap();
        s.device.media_release.add_permits(1);
        assert_eq!(waiter.await.unwrap().status, DeliveryStatus::Submitted);
        assert_eq!(s.device.media_calls.load(Ordering::SeqCst), 1);
    }).await;
}
