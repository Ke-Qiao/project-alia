//! Media mocks prove ownership and durable ordering; the ledger is real PostgreSQL.
use alia_server::channels::adapter::AudioBuffer;
mod delivery_support;
use alia_core::delivery::{
    ObservationEnd, PlaybackIdentity, PlaybackProgress, SubmissionBackend, SubmissionBoundary,
    VoiceScope,
};
use alia_core::output_control::{OutputBinding, OutputRegistration};
use alia_server::channels::adapter::*;
use alia_server::modules::delivery::{AudioDelivery, DeliveryService, OutputProducer, types::*};
use alia_server::modules::platform::lifecycle::ModuleLifecycle;
use async_trait::async_trait;
use delivery_support::*;
use futures::StreamExt;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone, Copy)]
enum Behavior {
    Accept,
    Submitted,
    Unconfirmed,
    IgnoreCancel,
    ImmediateError,
    LocalUnknown,
    SlowLease,
}
struct Channel {
    owner: Uuid,
    scope: VoiceScope,
    backend: SubmissionBackend,
    registration: std::sync::OnceLock<OutputRegistration>,
    behavior: Behavior,
    entered: Semaphore,
    release: Semaphore,
    finished: AtomicUsize,
    calls: AtomicUsize,
    media: AtomicUsize,
    fences: Mutex<Vec<PlaybackIdentity>>,
    last_progress: Mutex<Option<PlaybackProgress>>,
    db: DatabaseConnection,
}
#[async_trait]
impl ModuleLifecycle for Channel {
    fn name(&self) -> &'static str {
        "delivery-test"
    }
}
impl Channel {
    async fn retire_model_scope(&self, target: &ChannelTarget, _: &str) -> anyhow::Result<()> {
        assert_eq!(target.user_id, self.owner.to_string());
        assert_eq!(target.voice, Some(self.scope));
        // Scope retirement is a separate barrier. Preserve each test's original
        // exact PlaybackIdentity checkpoint and its injected ACK failure.
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for Channel {
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
        assert_eq!(binding.scope, self.scope);
        let registration = OutputRegistration {
            binding,
            backend: self.backend.clone(),
            maximum_hold_ms: 60000,
            initiative_capture_gate_version: 0,
        };
        self.registration.set(registration.clone()).unwrap();
        Ok(registration)
    }

    async fn retire_voice_output(
        &self,
        request: &alia_core::output_control::OutputRetirementRequest,
    ) -> anyhow::Result<alia_core::output_control::OutputRetirementReceipt> {
        use alia_core::output_control::{OutputRetirementEvidence, OutputRetirementReceipt};
        request.validate()?;
        assert_eq!(request.scope, self.scope);
        let registration = self
            .registration
            .get()
            .expect("fixture output was not registered");
        assert_eq!(request.binding, Some(registration.binding));
        assert_eq!(request.expected_backend.as_ref(), Some(&self.backend));
        self.retire_model_scope(
            &ChannelTarget {
                user_id: self.owner.to_string(),
                voice: Some(self.scope),
            },
            "original delivery model retired",
        )
        .await?;
        // The fake owns a software scope gate, not a native write queue. This
        // proof leaves each exact media cancel's injected unknown ACK intact.
        let receipt = OutputRetirementReceipt {
            scope: self.scope,
            binding: request.binding,
            evidence: OutputRetirementEvidence::Settled {
                backend: self.backend.clone(),
                progress: vec![],
            },
        };
        receipt.validate(request)?;
        Ok(receipt)
    }
    fn reserve_playback(
        &self,
        target: &ChannelTarget,
        playback_id: Uuid,
    ) -> anyhow::Result<PlaybackIdentity> {
        Ok(PlaybackIdentity {
            scope: target.voice.expect("scoped playback required"),
            playback_id,
            playback_sequence: 1,
        })
    }
    async fn cancel_playback(
        &self,
        identity: PlaybackIdentity,
        reason: &str,
    ) -> anyhow::Result<PlaybackProgress> {
        self.fences.lock().unwrap().push(identity);
        if matches!(
            self.behavior,
            Behavior::Unconfirmed | Behavior::IgnoreCancel
        ) {
            anyhow::bail!("no fence ACK");
        }
        let mut progress = self
            .last_progress
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no device checkpoint"))?;
        anyhow::ensure!(progress.identity == identity, "identity mismatch");
        if progress.observation_end.is_none() {
            progress.receipt_sequence += 1;
            progress.observation_end = Some(ObservationEnd::Cancelled {
                reason: reason.into(),
            });
        }
        *self.last_progress.lock().unwrap() = Some(progress.clone());
        Ok(progress)
    }
    async fn deliver_audio(
        &self,
        _: &ChannelTarget,
        identity: PlaybackIdentity,
        mut audio: AudioPayloadStream,
        observer: Arc<dyn PlaybackObserver>,
        cancel: CancellationToken,
    ) -> anyhow::Result<AudioDeliveryReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        observer.start(24000, 1).await?;
        if matches!(self.behavior, Behavior::SlowLease) {
            // Force overlap with the five-second heartbeat. The supervisor
            // must keep polling this adapter so it can release the DB lock.
            let tx = self.db.begin().await?;
            tx.execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT privacy_epoch FROM memory_privacy_state WHERE user_id=$1 FOR UPDATE",
                [self.owner.into()],
            ))
            .await?;
            tokio::time::sleep(Duration::from_millis(5300)).await;
            tx.commit().await?;
        }
        while let Some(chunk) = audio.next().await {
            chunk?;
        }
        observer.offered(100).await?;
        self.media.fetch_add(1, Ordering::SeqCst);
        let mut progress = receipt(identity, 1, 60);
        *self.last_progress.lock().unwrap() = Some(progress.clone());
        observer.progress(progress.clone()).await?;
        self.entered.add_permits(1);
        if matches!(self.behavior, Behavior::ImmediateError) {
            anyhow::bail!("injected connection loss after accepted prefix");
        }
        if matches!(self.behavior, Behavior::LocalUnknown) {
            self.finished.fetch_add(1, Ordering::SeqCst);
            return Ok(AudioDeliveryReport {
                progress: Some(progress),
                observation_end: ObservationEnd::Unknown {
                    reason: "transport lost observation; device state not yet queried".into(),
                },
            });
        }
        if matches!(self.behavior, Behavior::IgnoreCancel) {
            self.release.acquire().await.unwrap().forget();
        } else {
            tokio::select! {_=cancel.cancelled()=>{},p=self.release.acquire()=>p.unwrap().forget()}
        }
        self.finished.fetch_add(1, Ordering::SeqCst);
        if matches!(self.behavior, Behavior::Submitted | Behavior::SlowLease) {
            progress.receipt_sequence = 2;
            progress.accepted_source_frames = 100;
            progress.end_accepted = true;
            progress.submitted_source_frames = Some(100);
            progress.backend = Some(self.backend.clone());
            progress.observation_end = Some(ObservationEnd::SubmissionFinished);
            *self.last_progress.lock().unwrap() = Some(progress.clone());
            observer.progress(progress.clone()).await?;
            return Ok(AudioDeliveryReport {
                observation_end: ObservationEnd::SubmissionFinished,
                progress: Some(progress),
            });
        }
        Ok(AudioDeliveryReport::unobserved("no final observed receipt"))
    }
}
fn channel(f: &Fixture, behavior: Behavior) -> Arc<Channel> {
    Arc::new(Channel {
        owner: f.user,
        scope: identity().scope,
        backend: SubmissionBackend {
            instance_id: Uuid::new_v4(),
            boundary: SubmissionBoundary::CpalCallbackBuffer,
            output_sample_rate_hz: 48000,
            output_channels: 2,
        },
        registration: std::sync::OnceLock::new(),
        behavior,
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
        finished: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
        media: AtomicUsize::new(0),
        fences: Mutex::new(Vec::new()),
        last_progress: Mutex::new(None),
        db: f.db.clone(),
    })
}
async fn prepared_output(
    service: &Arc<DeliveryService>,
    f: &Fixture,
    channel: &Arc<Channel>,
) -> (ResponseRecord, Arc<OutputProducer>) {
    let mut input = f.response().await;
    input.content["voice_scope"] = json!(channel.scope);
    let producer = service
        .begin_voice_output(
            f.user,
            input.key.request_id,
            channel.scope,
            channel.clone(),
            CancellationToken::new(),
            None,
        )
        .unwrap();
    let response = service.prepare(input).await.unwrap();
    producer.bind(response.clone()).await.unwrap();
    (response, producer)
}
fn request(response: Uuid, channel: Arc<Channel>, cancel: CancellationToken) -> AudioDelivery {
    AudioDelivery {
        response_id: response,
        output_key: "speech:0".into(),
        content: json!({"text":"hello"}),
        target: ChannelTarget {
            user_id: channel.owner.to_string(),
            voice: Some(channel.scope),
        },
        channel,
        audio: Box::pin(futures::stream::iter([Ok(AudioPayload {
            data: AudioBuffer::input(vec![0; 200]),
            sample_rate: 24000,
            channels: 1,
            format: AudioFormat::Pcm16,
        })])),
        cancel,
    }
}
async fn wait_entered(channel: &Channel) {
    tokio::time::timeout(Duration::from_secs(3), channel.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}
async fn wait_terminal(service: &DeliveryService, owner: Uuid, response: Uuid) -> DeliveryRecord {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let records = service.list_deliveries(owner, response).await.unwrap();
            if let Some(record) = records.into_iter().find(|r| r.status.is_terminal()) {
                break record;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn dropping_caller_preserves_supervisor_and_shutdown_uses_exact_identity() {
    with_db(|f| async move {
        let stop = CancellationToken::new();
        let service = DeliveryService::new(f.repo(), &stop);
        service.start().await.unwrap();
        let channel = channel(&f, Behavior::Accept);
        let (response, _producer) = prepared_output(&service, &f, &channel).await;
        let input = request(
            response.response_id,
            channel.clone(),
            CancellationToken::new(),
        );
        let expected_scope = input.target.voice;
        let owner = service.clone();
        let caller = tokio::spawn(async move { owner.deliver_audio(input).await });
        wait_entered(&channel).await;
        caller.abort();
        let _ = caller.await;
        assert_eq!(channel.finished.load(Ordering::SeqCst), 0);
        assert!(service.shutdown_pending());
        service.shutdown().await.unwrap();
        assert!(!service.shutdown_pending());
        assert_eq!(channel.finished.load(Ordering::SeqCst), 1);
        let record = service
            .list_deliveries(f.user, response.response_id)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(record.status, DeliveryStatus::Cancelled);
        assert_eq!(record.accepted_frames, 60);
        assert_eq!(record.presented_frames, None);
        assert_eq!(
            channel.fences.lock().unwrap().as_slice(),
            &[record.identity.unwrap()]
        );
        assert_eq!(Some(record.identity.unwrap().scope), expected_scope);
    })
    .await;
}

#[tokio::test]
async fn submitted_receipt_is_terminal_without_post_speech_evidence() {
    with_db(|f| async move {
        let service = DeliveryService::new(f.repo(), &CancellationToken::new());
        service.start().await.unwrap();
        let channel = channel(&f, Behavior::Submitted);
        let (response, _producer) = prepared_output(&service, &f, &channel).await;
        channel.release.add_permits(1);
        let record = service
            .deliver_audio(request(
                response.response_id,
                channel.clone(),
                CancellationToken::new(),
            ))
            .await
            .unwrap();
        assert_eq!(record.status, DeliveryStatus::Submitted);
        assert_eq!(record.presented_frames, None);
        assert_eq!(channel.calls.load(Ordering::SeqCst), 1);
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn lease_wait_does_not_pause_adapter_holding_source_transaction() {
    with_db(|f| async move {
        let service = DeliveryService::new(f.repo(), &CancellationToken::new());
        service.start().await.unwrap();
        let channel = channel(&f, Behavior::SlowLease);
        let (response, _producer) = prepared_output(&service, &f, &channel).await;
        channel.release.add_permits(1);
        let record = tokio::time::timeout(
            Duration::from_secs(8),
            service.deliver_audio(request(
                response.response_id,
                channel.clone(),
                CancellationToken::new(),
            )),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(record.status, DeliveryStatus::Submitted);
        assert!(channel.fences.lock().unwrap().is_empty());
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn unconfirmed_cancel_is_unknown_and_cannot_replay() {
    with_db(|f| async move {
        let service = DeliveryService::new(f.repo(), &CancellationToken::new());
        service.start().await.unwrap();
        let channel = channel(&f, Behavior::Unconfirmed);
        let (response, _producer) = prepared_output(&service, &f, &channel).await;
        let input = request(
            response.response_id,
            channel.clone(),
            CancellationToken::new(),
        );
        let target = input.target.clone();
        let owner = service.clone();
        let caller = tokio::spawn(async move { owner.deliver_audio(input).await });
        wait_entered(&channel).await;
        service
            .cancel_response(f.user, response.response_id)
            .await
            .unwrap();
        let record = caller.await.unwrap().unwrap();
        assert_eq!(record.status, DeliveryStatus::Unknown);
        assert_eq!(record.accepted_frames, 60);
        let mut retry = request(
            response.response_id,
            channel.clone(),
            CancellationToken::new(),
        );
        retry.target = target;
        assert_eq!(
            service.deliver_audio(retry).await.unwrap().delivery_id,
            record.delivery_id
        );
        assert_eq!(channel.calls.load(Ordering::SeqCst), 1);
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn local_transport_unknown_preserves_device_sequence_and_uses_independent_final_checkpoint() {
    with_db(|f| async move {
        let service = DeliveryService::new(f.repo(), &CancellationToken::new());
        service.start().await.unwrap();
        let channel = channel(&f, Behavior::LocalUnknown);
        let (response, _producer) = prepared_output(&service, &f, &channel).await;
        let record = tokio::time::timeout(
            Duration::from_secs(4),
            service.deliver_audio(request(
                response.response_id,
                channel.clone(),
                CancellationToken::new(),
            )),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(record.status, DeliveryStatus::Cancelled);
        assert_eq!(record.accepted_frames, 60);
        assert_eq!(record.receipt_sequence, 2);
        assert_eq!(channel.calls.load(Ordering::SeqCst), 1);
        assert_eq!(channel.finished.load(Ordering::SeqCst), 1);
        let events = service
            .response_events(f.user, response.response_id, 0, 256)
            .await
            .unwrap();
        assert!(
            !events
                .iter()
                .any(|event| event.kind == "unknown" || event.kind == "late_checkpoint")
        );
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn old_epoch_heartbeat_cancels_original_playback_and_preserves_prefix() {
    with_db(|f| async move {
        let service = DeliveryService::new(f.repo(), &CancellationToken::new());
        service.start().await.unwrap();
        let channel = channel(&f, Behavior::Accept);
        let (response, _producer) = prepared_output(&service, &f, &channel).await;
        let owner = service.clone();
        let id = response.response_id;
        let output = channel.clone();
        let caller = tokio::spawn(async move {
            owner
                .deliver_audio(request(id, output, CancellationToken::new()))
                .await
        });
        wait_entered(&channel).await;
        f.sql(&format!(
            "UPDATE memory_privacy_state SET privacy_epoch=1 WHERE user_id='{}'",
            f.user
        ))
        .await;
        let record = tokio::time::timeout(Duration::from_secs(7), caller)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(record.status, DeliveryStatus::Cancelled);
        assert_eq!(record.accepted_frames, 60);
        assert_eq!(
            channel.fences.lock().unwrap().as_slice(),
            &[record.identity.unwrap()]
        );
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn cancellation_timeout_persists_unknown_but_retains_actual_adapter_owner() {
    with_db(|f| async move {
        let service = DeliveryService::new(f.repo(), &CancellationToken::new());
        service.start().await.unwrap();
        let channel = channel(&f, Behavior::IgnoreCancel);
        let (response, _producer) = prepared_output(&service, &f, &channel).await;
        let owner = service.clone();
        let id = response.response_id;
        let output = channel.clone();
        let caller = tokio::spawn(async move {
            owner
                .deliver_audio(request(id, output, CancellationToken::new()))
                .await
        });
        wait_entered(&channel).await;
        service
            .cancel_response(f.user, response.response_id)
            .await
            .unwrap();
        let record = wait_terminal(&service, f.user, id).await;
        assert_eq!(record.status, DeliveryStatus::Unknown);
        assert!(!caller.is_finished());
        assert_eq!(channel.finished.load(Ordering::SeqCst), 0);
        let shutdown = tokio::time::timeout(Duration::from_millis(30), service.shutdown()).await;
        assert!(shutdown.is_err());
        assert!(service.shutdown_pending());
        channel.release.add_permits(1);
        assert_eq!(
            caller.await.unwrap().unwrap().status,
            DeliveryStatus::Unknown
        );
        assert!(
            service.shutdown().await.is_err(),
            "the earlier cancellation deadline failure remains visible"
        );
        assert!(!service.shutdown_pending());
        assert_eq!(channel.finished.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn closed_admission_and_already_cancelled_input_send_no_media() {
    with_db(|f| async move {
        let service = DeliveryService::new(f.repo(), &CancellationToken::new());
        service.start().await.unwrap();
        let channel = channel(&f, Behavior::Accept);
        let (response, _producer) = prepared_output(&service, &f, &channel).await;
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let record = service
            .deliver_audio(request(response.response_id, channel.clone(), cancelled))
            .await
            .unwrap();
        assert_eq!(record.status, DeliveryStatus::Cancelled);
        assert_eq!(channel.calls.load(Ordering::SeqCst), 0);
        service.close_admission();
        assert!(
            service
                .deliver_audio(request(
                    response.response_id,
                    channel.clone(),
                    CancellationToken::new()
                ))
                .await
                .is_err()
        );
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn failed_durable_offer_prevents_external_audio_and_preserves_zero_bound() {
    with_db(|f|async move{
 let service=DeliveryService::new(f.repo(),&CancellationToken::new());service.start().await.unwrap();let channel = channel(&f, Behavior::Accept);
        let (response, _producer) = prepared_output(&service, &f, &channel).await;
 f.sql("CREATE FUNCTION fail_offer() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='offered' THEN RAISE EXCEPTION 'injected offered audit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_offer BEFORE INSERT ON delivery_events FOR EACH ROW EXECUTE FUNCTION fail_offer()").await;
 let record=service.deliver_audio(request(response.response_id,channel.clone(),CancellationToken::new())).await.unwrap();assert_eq!(channel.calls.load(Ordering::SeqCst),1);assert_eq!(channel.media.load(Ordering::SeqCst),0);assert_eq!(record.offered_frames,0);assert_eq!(record.accepted_frames,0);assert_eq!(record.presented_frames,None);assert!(record.status.is_terminal());service.shutdown().await.unwrap();
}).await;
}

#[tokio::test]
async fn completed_error_future_is_not_repolled_and_known_prefix_survives() {
    with_db(|f| async move {
        let service = DeliveryService::new(f.repo(), &CancellationToken::new());
        service.start().await.unwrap();
        let channel = channel(&f, Behavior::ImmediateError);
        let (response, _producer) = prepared_output(&service, &f, &channel).await;
        let record = tokio::time::timeout(
            Duration::from_secs(3),
            service.deliver_audio(request(
                response.response_id,
                channel.clone(),
                CancellationToken::new(),
            )),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(record.accepted_frames, 60);
        assert_eq!(record.presented_frames, None);
        assert_eq!(record.status, DeliveryStatus::Cancelled);
        assert_eq!(
            channel.fences.lock().unwrap().as_slice(),
            &[record.identity.unwrap()]
        );
        service.shutdown().await.unwrap();
        assert!(!service.shutdown_pending());
    })
    .await;
}

#[tokio::test]
async fn another_instance_cancels_observing_output_through_durable_control() {
    with_db(|f| async move {
        let a = DeliveryService::new(f.repo(), &CancellationToken::new());
        let b = DeliveryService::new(f.repo(), &CancellationToken::new());
        a.start().await.unwrap();
        b.start().await.unwrap();
        let channel = channel(&f, Behavior::Accept);
        let (response, _producer) = prepared_output(&a, &f, &channel).await;
        let owner = a.clone();
        let id = response.response_id;
        let output = channel.clone();
        let caller = tokio::spawn(async move {
            owner
                .deliver_audio(request(id, output, CancellationToken::new()))
                .await
        });
        wait_entered(&channel).await;
        b.cancel_response(f.user, id).await.unwrap();
        let record = tokio::time::timeout(Duration::from_secs(7), caller)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(record.status, DeliveryStatus::Cancelled);
        assert_eq!(
            channel.fences.lock().unwrap().as_slice(),
            &[record.identity.unwrap()]
        );
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn recovered_unknown_lease_stops_the_returning_original_owner() {
    with_db(|f|async move{
 let service=DeliveryService::new(f.repo(),&CancellationToken::new());service.start().await.unwrap();let channel = channel(&f, Behavior::Accept);
        let (response, _producer) = prepared_output(&service, &f, &channel).await;let owner=service.clone();let id=response.response_id;let output=channel.clone();let caller=tokio::spawn(async move{owner.deliver_audio(request(id,output,CancellationToken::new())).await});wait_entered(&channel).await;f.sql("UPDATE deliveries SET lease_expires_at=clock_timestamp()-interval '1 second' WHERE status IN ('sending','observing')").await;assert_eq!(f.repo().recover_expired(1).await.unwrap(),1);let record=tokio::time::timeout(Duration::from_secs(7),caller).await.unwrap().unwrap().unwrap();assert_eq!(record.status,DeliveryStatus::Unknown);assert_eq!(record.accepted_frames,60);assert_eq!(channel.fences.lock().unwrap().as_slice(),&[record.identity.unwrap()]);service.shutdown().await.unwrap();
}).await;
}
