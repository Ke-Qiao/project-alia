use super::*;
use crate::module::lifecycle::ModuleLifecycle;
use crate::module::playback_session::{AudioSubmission, CaptureControlBarrier, PlaybackControl};
use crate::module::traits::AudioPlaybackBackend;
use crate::storage::EmbeddedStore;
use alia_core::output_control::{
    BoundControlCommand, BoundControlReceipt, OutputBinding, OutputRegistration,
};
use alia_core::playback_control::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct Device {
    state: Mutex<(u64, Vec<i16>)>,
    control: Mutex<Option<Arc<PlaybackControl>>>,
    backend: alia_core::delivery::SubmissionBackend,
    changed: Condvar,
    capacity: usize,
    submissions: Mutex<HashMap<PlaybackIdentity, Arc<AudioSubmission>>>,
    queued: Mutex<Vec<(PlaybackIdentity, Vec<i16>)>>,
    blocked: AtomicUsize,
    hold_end: AtomicBool,
    end_entered: AtomicBool,
}
#[tonic::async_trait]
impl ModuleLifecycle for Device {
    fn name(&self) -> &'static str {
        "test-playback"
    }
}
impl AudioPlaybackBackend for Device {
    fn playback_control(
        &self,
        scope: &VoiceScope,
        generation: u64,
    ) -> anyhow::Result<Arc<PlaybackControl>> {
        anyhow::ensure!(self.is_playback_current(generation), "native owner gone");
        let mut control = self.control.lock().unwrap();
        if let Some(control) = control.as_ref() {
            anyhow::ensure!(
                control.scope == *scope && control.generation == generation,
                "scope changed"
            );
            return Ok(control.clone());
        }
        let created = PlaybackControl::new(*scope, generation, self.backend.clone())?;
        *control = Some(created.clone());
        Ok(created)
    }
    fn apply_playback_control(
        &self,
        control: &Arc<PlaybackControl>,
        command: &ControlCommand,
    ) -> anyhow::Result<ControlReceipt> {
        anyhow::ensure!(
            self.is_playback_current(control.generation),
            "native owner gone"
        );
        let receipt = control.apply(command);
        self.changed.notify_all();
        Ok(receipt)
    }
    fn open_submission(
        &self,
        identity: PlaybackIdentity,
        rate: u32,
        generation: u64,
    ) -> anyhow::Result<Arc<AudioSubmission>> {
        let control = self.playback_control(&identity.scope, generation)?;
        let submission = AudioSubmission::with_control(
            identity,
            generation,
            rate,
            self.backend.clone(),
            Some(control),
        )?;
        self.submissions
            .lock()
            .unwrap()
            .insert(identity, submission.clone());
        Ok(submission)
    }
    fn submit_audio(
        &self,
        submission: &Arc<AudioSubmission>,
        offset: u64,
        pcm: &[i16],
    ) -> anyhow::Result<bool> {
        let mut state = self.state.lock().unwrap();
        while state.0 == submission.generation
            && submission.is_live()
            && state.1.len() >= self.capacity
        {
            self.blocked.fetch_add(1, Ordering::SeqCst);
            state = self.changed.wait(state).unwrap();
            self.blocked.fetch_sub(1, Ordering::SeqCst);
        }
        if state.0 != submission.generation || !submission.is_live() {
            return Ok(false);
        }
        submission.accept(offset, pcm.len() as u64)?;
        self.queued
            .lock()
            .unwrap()
            .push((submission.identity, pcm.to_vec()));
        state.1.extend_from_slice(pcm);
        Ok(true)
    }
    fn end_submission(
        &self,
        submission: &Arc<AudioSubmission>,
        total: u64,
    ) -> anyhow::Result<bool> {
        let accepted = submission.accept_end(total)?;
        self.end_entered.store(true, Ordering::Release);
        let mut state = self.state.lock().unwrap();
        while self.hold_end.load(Ordering::Acquire) {
            state = self.changed.wait(state).unwrap();
        }
        Ok(accepted)
    }
    fn cancel_submission(
        &self,
        submission: &Arc<AudioSubmission>,
        reason: &str,
    ) -> anyhow::Result<bool> {
        let mut state = self.state.lock().unwrap();
        if state.0 != submission.generation {
            return Ok(false);
        }
        self.hold_end.store(false, Ordering::Release);
        submission.terminate(ObservationEnd::Cancelled {
            reason: reason.into(),
        });
        let mut queued = self.queued.lock().unwrap();
        queued.retain(|(identity, _)| *identity != submission.identity);
        state.1 = queued
            .iter()
            .flat_map(|(_, pcm)| pcm.iter().copied())
            .collect();
        self.changed.notify_all();
        Ok(true)
    }
    fn play(&self, _: &[i16], _: u32) -> anyhow::Result<()> {
        unreachable!()
    }
    fn stop_playback(&self) -> anyhow::Result<()> {
        unreachable!()
    }
    fn is_available(&self) -> bool {
        true
    }
    fn begin_scoped_playback(&self) -> anyhow::Result<u64> {
        let mut state = self.state.lock().unwrap();
        state.0 += 1;
        if let Some(control) = self.control.lock().unwrap().take() {
            control.retire(RetiredReason::Cancelled);
        }
        state.1.clear();
        self.queued.lock().unwrap().clear();
        self.changed.notify_all();
        Ok(state.0)
    }
    fn play_if_current(&self, pcm: &[i16], _: u32, generation: u64) -> anyhow::Result<bool> {
        let mut state = self.state.lock().unwrap();
        while state.0 == generation && state.1.len() >= self.capacity {
            state = self.changed.wait(state).unwrap();
        }
        if state.0 != generation {
            return Ok(false);
        }
        state.1.extend_from_slice(pcm);
        Ok(true)
    }
    fn stop_if_current(&self, generation: u64) -> anyhow::Result<bool> {
        let mut state = self.state.lock().unwrap();
        if state.0 != generation {
            return Ok(false);
        }
        state.0 += 1;
        if let Some(control) = self.control.lock().unwrap().take() {
            control.retire(RetiredReason::Cancelled);
        }
        state.1.clear();
        self.queued.lock().unwrap().clear();
        self.changed.notify_all();
        Ok(true)
    }
    fn is_playback_current(&self, generation: u64) -> bool {
        self.state.lock().unwrap().0 == generation
    }
}

impl Device {
    /// Explicit simulated callback submission, never a sleep-based completion.
    fn submit_frames(&self, id: &str, frames: usize) {
        let mut state = self.state.lock().unwrap();
        let mut queued = self.queued.lock().unwrap();
        let submission = self
            .submissions
            .lock()
            .unwrap()
            .values()
            .find(|s| s.identity.playback_id.to_string() == id)
            .unwrap()
            .clone();
        let write = submission
            .try_begin_output()
            .expect("simulated callback must respect the original gate");
        let mut remaining = frames;
        for (identity, pcm) in queued.iter_mut() {
            if identity.playback_id.to_string() == id {
                let count = remaining.min(pcm.len());
                pcm.drain(..count);
                remaining -= count;
            }
        }
        assert_eq!(remaining, 0);
        state.1 = queued
            .iter()
            .flat_map(|(_, pcm)| pcm.iter().copied())
            .collect();
        write.finish(frames as u64);
        self.changed.notify_all();
    }
}

struct Fixture {
    client: proto::playback_client::PlaybackClient<tonic::transport::Channel>,
    scope: VoiceScope,
    capture: CaptureControlBarrier,
    session: Arc<PlaybackSession>,
    device: Arc<Device>,
    binding: Arc<BindingManager>,
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new(capacity: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = EmbeddedStore::open(dir.path().join("state.sqlite"))
            .await
            .unwrap();
        let binding = Arc::new(BindingManager::new(store, 3, 60));
        let key = ed25519_dalek::SigningKey::from_bytes(&[27; 32]);
        let public = key.verifying_key().to_bytes();
        let issued = alia_transport::current_unix_timestamp().unwrap();
        let payload = alia_transport::bind_payload(
            "server",
            "server",
            "http://localhost:1234",
            &public,
            issued,
        );
        binding
            .bind(
                "server",
                "server",
                "http://localhost:1234",
                &public,
                &alia_transport::sign_payload(&key, &payload),
                issued,
            )
            .await
            .unwrap();
        let device = Arc::new(Device {
            state: Mutex::new((0, Vec::new())),
            control: Mutex::new(None),
            backend: alia_core::delivery::SubmissionBackend {
                instance_id: Uuid::new_v4(),
                boundary: SubmissionBoundary::CpalCallbackBuffer,
                output_sample_rate_hz: 16000,
                output_channels: 1,
            },
            changed: Condvar::new(),
            capacity,
            submissions: Mutex::new(HashMap::new()),
            queued: Mutex::new(Vec::new()),
            blocked: AtomicUsize::new(0),
            hold_end: AtomicBool::new(false),
            end_entered: AtomicBool::new(false),
        });
        let session = Arc::new(PlaybackSession::new(device.clone()));
        let scope = VoiceScope {
            device_boot_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            generation_epoch: 1,
        };
        let capture = CaptureControlBarrier::default();
        session
            .accept_capture(&capture, 0, true, || Ok(()))
            .unwrap();
        session
            .activate_with_capture(&scope, capture.clone(), MAX_HOLD_MS)
            .unwrap();
        let revocation = session.clone();
        binding
            .install_output_revocation(Arc::new(move || revocation.revoke_all().unwrap()))
            .unwrap();
        let service = proto::playback_server::PlaybackServer::new(PlaybackService::new(
            session.clone(),
            binding.clone(),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let server = tokio::spawn(async move {
            let incoming = async_stream::stream! { loop { yield listener.accept().await.map(|(stream, _)| stream); } };
            tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming_shutdown(incoming, stop.cancelled())
                .await
                .unwrap();
        });
        let mut client = proto::playback_client::PlaybackClient::connect(endpoint)
            .await
            .unwrap();
        client
            .register_output(authenticated(proto::RegisterOutputRequest {
                binding: Some(codec::encode_binding(output_binding(scope))),
                contract_version: PLAYBACK_CONTRACT_VERSION,
            }))
            .await
            .unwrap();
        Self {
            client,
            scope,
            capture,
            session,
            device,
            binding,
            shutdown,
            server,
            _dir: dir,
        }
    }

    async fn register(&mut self, scope: VoiceScope) -> OutputRegistration {
        codec::decode_registration(
            &self
                .client
                .register_output(authenticated(proto::RegisterOutputRequest {
                    binding: Some(codec::encode_binding(output_binding(scope))),
                    contract_version: PLAYBACK_CONTRACT_VERSION,
                }))
                .await
                .unwrap()
                .into_inner(),
        )
        .unwrap()
    }
    async fn command(&mut self, command: &BoundControlCommand) -> BoundControlReceipt {
        codec::decode_control_receipt(
            &self
                .client
                .control(authenticated(codec::encode_control_request(command)))
                .await
                .unwrap()
                .into_inner(),
            command,
        )
        .unwrap()
    }
    async fn observe(&mut self, command: &BoundControlCommand) -> BoundControlReceipt {
        codec::decode_control_receipt(
            &self
                .client
                .observe_control(authenticated(codec::encode_control_request(command)))
                .await
                .unwrap()
                .into_inner(),
            command,
        )
        .unwrap()
    }
    fn pause(&self, reason: HoldReason, budget_ms: u64) -> BoundControlCommand {
        BoundControlCommand {
            binding: output_binding(self.scope),
            expected_backend: self.device.backend.clone(),
            command: ControlCommand {
                scope: self.scope,
                hold_id: Uuid::new_v4(),
                command_id: Uuid::new_v4(),
                sequence: 1,
                action: ControlAction::Pause {
                    reason,
                    through_sequence: 0,
                    budget_ms,
                },
            },
        }
    }
    async fn finish(self) {
        self.session.stop().await.unwrap();
        drop(self.client);
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.server)
            .await
            .unwrap()
            .unwrap();
    }
}

fn authenticated<T>(body: T) -> Request<T> {
    authenticated_for(body, Duration::from_secs(60))
}
fn authenticated_for<T>(body: T, ttl: Duration) -> Request<T> {
    let key = ed25519_dalek::SigningKey::from_bytes(&[27; 32]);
    let token = alia_transport::mint_auth_token(
        &key,
        "server",
        alia_transport::EMBEDDED_CONTROL_SCOPE,
        ttl,
    )
    .unwrap();
    let mut request = Request::new(body);
    request
        .metadata_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

fn frame(
    scope: VoiceScope,
    id: &str,
    playback_sequence: u64,
    mut payload: playback_frame::Payload,
) -> proto::PlaybackFrame {
    if let playback_frame::Payload::Start(start) = &mut payload {
        start.output_binding = Some(codec::encode_binding(output_binding(scope)));
    }
    proto::PlaybackFrame {
        scope: Some(encode_scope(scope)),
        playback_id: id.into(),
        playback_sequence,
        payload: Some(payload),
    }
}
fn output_binding(scope: VoiceScope) -> OutputBinding {
    OutputBinding {
        scope,
        response_id: scope.session_id,
        context_id: scope.device_boot_id,
        server_instance_id: Uuid::from_u128(901),
    }
}
fn start() -> playback_frame::Payload {
    playback_frame::Payload::Start(proto::PlaybackStart {
        contract_version: PLAYBACK_CONTRACT_VERSION,
        output_binding: None,
        sample_rate_hz: 16000,
        channels: 1,
        source: "tts".into(),
    })
}
fn audio(sequence: u64, sample: i16) -> playback_frame::Payload {
    playback_frame::Payload::Audio(proto::PlaybackAudio {
        sequence,
        sample_offset: sequence,
        pcm_s16le: sample.to_le_bytes().to_vec(),
    })
}
fn end(samples: u64) -> playback_frame::Payload {
    playback_frame::Payload::End(proto::PlaybackEnd {
        total_samples: samples,
    })
}

async fn receipt_from(
    stream: &mut tonic::Streaming<proto::PlaybackReceipt>,
) -> proto::PlaybackReceipt {
    let receipt = tokio::time::timeout(Duration::from_secs(3), stream.message())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(receipt.submitted_samples.is_some() && receipt.presented_samples.is_none());
    assert_eq!(receipt.contract_version, PLAYBACK_CONTRACT_VERSION);
    assert_ne!(receipt.state, proto::PlaybackState::Completed as i32);
    receipt
}

async fn until(
    stream: &mut tonic::Streaming<proto::PlaybackReceipt>,
    predicate: impl Fn(&proto::PlaybackReceipt) -> bool,
) -> proto::PlaybackReceipt {
    loop {
        let receipt = receipt_from(stream).await;
        if predicate(&receipt) {
            return receipt;
        }
    }
}

#[tokio::test]
async fn grpc_fence_unblocks_full_media_queue_and_old_fence_cannot_stop_new_epoch() {
    let mut f = Fixture::new(1).await;
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel(4);
    tx.send(frame(f.scope, &id, 1, start())).await.unwrap();
    tx.send(frame(f.scope, &id, 1, audio(0, 1))).await.unwrap();
    let mut stream = f
        .client
        .push(authenticated(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(receipt_from(&mut stream).await.accepted_samples, 0);
    assert_eq!(receipt_from(&mut stream).await.accepted_samples, 1);
    tx.send(frame(f.scope, &id, 1, audio(1, 2))).await.unwrap();
    let newer = VoiceScope {
        generation_epoch: 2,
        ..f.scope
    };
    let reply = f
        .client
        .fence(authenticated(proto::FenceRequest {
            scope: Some(encode_scope(newer)),
            reason: "interrupt".into(),
            close_session: false,
            playback_id: None,
            playback_sequence: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(reply.applied);
    assert_eq!(
        receipt_from(&mut stream).await.state,
        proto::PlaybackState::Cancelled as i32
    );
    drop(stream);
    drop(tx);
    let next_id = Uuid::new_v4().to_string();
    f.register(newer).await;
    let frames = [
        frame(newer, &next_id, 2, start()),
        frame(newer, &next_id, 2, audio(0, 3)),
        frame(newer, &next_id, 2, end(1)),
    ];
    let mut next = f
        .client
        .push(authenticated(tokio_stream::iter(frames)))
        .await
        .unwrap()
        .into_inner();
    receipt_from(&mut next).await;
    receipt_from(&mut next).await;
    assert!(receipt_from(&mut next).await.end_accepted);
    let stale = f
        .client
        .fence(authenticated(proto::FenceRequest {
            scope: Some(encode_scope(f.scope)),
            reason: "late old interrupt".into(),
            close_session: false,
            playback_id: None,
            playback_sequence: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!stale.applied);
    assert_eq!(f.device.state.lock().unwrap().1, vec![3]);
    f.device.submit_frames(&next_id, 1);
    let done = until(&mut next, |r| r.observation_end != 0).await;
    assert_eq!(
        done.observation_end,
        proto::ObservationEnd::SubmissionFinished as i32
    );
    assert!(next.message().await.unwrap().is_none());
    drop(next);
    f.finish().await;
}

#[tokio::test]
async fn grpc_rejects_media_after_end_and_revocation_retires_queued_audio() {
    let mut f = Fixture::new(10).await;
    let id = Uuid::new_v4().to_string();
    let frames = [
        frame(f.scope, &id, 1, start()),
        frame(f.scope, &id, 1, audio(0, 1)),
        frame(f.scope, &id, 1, end(1)),
        frame(f.scope, &id, 1, audio(1, 2)),
    ];
    let mut stream = f
        .client
        .push(authenticated(tokio_stream::iter(frames)))
        .await
        .unwrap()
        .into_inner();
    receipt_from(&mut stream).await;
    receipt_from(&mut stream).await;
    let failed = receipt_from(&mut stream).await;
    assert_eq!(failed.state, proto::PlaybackState::Failed as i32);
    assert!(!failed.end_accepted);
    assert!(f.device.state.lock().unwrap().1.is_empty());
    drop(stream);
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel(4);
    tx.send(frame(f.scope, &id, 2, start())).await.unwrap();
    tx.send(frame(f.scope, &id, 2, audio(0, 3))).await.unwrap();
    let mut stream = f
        .client
        .push(authenticated(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
        .await
        .unwrap()
        .into_inner();
    receipt_from(&mut stream).await;
    receipt_from(&mut stream).await;
    tx.send(frame(f.scope, &id, 2, end(1))).await.unwrap();
    drop(tx);
    until(&mut stream, |r| r.end_accepted).await;
    f.binding.unbind("server").await.unwrap();
    assert_eq!(
        receipt_from(&mut stream).await.state,
        proto::PlaybackState::Cancelled as i32
    );
    assert!(f.device.state.lock().unwrap().1.is_empty());
    drop(stream);
    f.finish().await;
}

#[test]
fn receipts_never_invent_device_presentation_evidence() {
    let invalid = proto::VoiceScope {
        device_boot_id: Uuid::nil().to_string(),
        session_id: Uuid::new_v4().to_string(),
        generation_epoch: 1,
    };
    assert!(decode_scope(Some(&invalid)).is_err());
    assert!(decode_scope(None).is_err());
    let valid = VoiceScope {
        device_boot_id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        generation_epoch: 2,
    };
    assert_eq!(decode_scope(Some(&encode_scope(valid))).unwrap(), valid);
}

#[tokio::test]
async fn end_preserves_tail_observation_and_exact_cancel_does_not_touch_fifo_successor() {
    let mut f = Fixture::new(20).await;
    let first_id = Uuid::new_v4().to_string();
    let mut first = f
        .client
        .push(authenticated(tokio_stream::iter([
            frame(f.scope, &first_id, 1, start()),
            frame(f.scope, &first_id, 1, audio(0, 11)),
            frame(f.scope, &first_id, 1, audio(1, 12)),
            frame(f.scope, &first_id, 1, end(2)),
        ])))
        .await
        .unwrap()
        .into_inner();
    let accepted = until(&mut first, |r| r.end_accepted).await;
    assert_eq!(accepted.submitted_samples, Some(0));
    assert_eq!(accepted.observation_end, 0);
    f.device.submit_frames(&first_id, 1);
    let partial = until(&mut first, |r| r.submitted_samples == Some(1)).await;
    assert!(partial.receipt_sequence > accepted.receipt_sequence);
    assert_eq!(partial.backend, accepted.backend);
    let second_id = Uuid::new_v4().to_string();
    let mut second = f
        .client
        .push(authenticated(tokio_stream::iter([
            frame(f.scope, &second_id, 2, start()),
            frame(f.scope, &second_id, 2, audio(0, 21)),
            frame(f.scope, &second_id, 2, end(1)),
        ])))
        .await
        .unwrap()
        .into_inner();
    until(&mut second, |r| r.end_accepted).await;
    let ack = f
        .client
        .fence(authenticated(proto::FenceRequest {
            scope: Some(encode_scope(f.scope)),
            playback_id: Some(first_id.clone()),
            playback_sequence: Some(1),
            close_session: false,
            reason: "cancel only first delivery".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(ack.applied);
    assert_eq!(ack.playback_id, Some(first_id.clone()));
    assert_eq!(ack.playback_sequence, Some(1));
    assert_eq!(f.session.active_scope().unwrap(), Some(f.scope));
    assert_eq!(f.device.state.lock().unwrap().1, vec![21]);
    let retired = until(&mut first, |r| r.observation_end != 0).await;
    assert_eq!(
        retired.observation_end,
        proto::ObservationEnd::Cancelled as i32
    );
    assert_eq!(retired.submitted_samples, Some(1));
    assert!(retired.receipt_sequence > partial.receipt_sequence);
    assert!(first.message().await.unwrap().is_none());
    f.device.submit_frames(&second_id, 1);
    let submitted = until(&mut second, |r| r.observation_end != 0).await;
    assert_eq!(submitted.submitted_samples, Some(1));
    assert_eq!(
        submitted.observation_end,
        proto::ObservationEnd::SubmissionFinished as i32
    );
    assert!(second.message().await.unwrap().is_none());
    drop(first);
    drop(second);
    f.finish().await;
}

#[tokio::test]
async fn dropping_receipts_unblocks_and_joins_native_media_admission() {
    let mut f = Fixture::new(1).await;
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel(4);
    tx.send(frame(f.scope, &id, 1, start())).await.unwrap();
    tx.send(frame(f.scope, &id, 1, audio(0, 1))).await.unwrap();
    let mut stream = f
        .client
        .push(authenticated(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
        .await
        .unwrap()
        .into_inner();
    until(&mut stream, |r| r.accepted_samples == 1).await;
    tx.send(frame(f.scope, &id, 1, audio(1, 2))).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.device.blocked.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(stream);
    drop(tx);
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.device.blocked.load(Ordering::SeqCst) != 0
            || !f.device.state.lock().unwrap().1.is_empty()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), f.session.stop())
        .await
        .unwrap()
        .unwrap();
    assert!(
        f.session.activate(&f.scope).is_err(),
        "stopped runtime cannot reactivate"
    );
    assert!(
        f.session
            .begin(&f.scope, &Uuid::new_v4().to_string(), 2, 16000)
            .is_err()
    );
    let tracker = f
        .device
        .submissions
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    assert!(matches!(
        tracker.snapshot(100).observation_end,
        Some(ObservationEnd::Unknown { .. })
    ));
    f.finish().await;
}

#[tokio::test]
async fn old_contract_is_rejected_before_any_media_admission() {
    let mut f = Fixture::new(8).await;
    let mut old = start();
    let playback_frame::Payload::Start(ref mut value) = old else {
        unreachable!()
    };
    value.contract_version = 1;
    let id = Uuid::new_v4().to_string();
    let mut stream = f
        .client
        .push(authenticated(tokio_stream::iter([
            frame(f.scope, &id, 1, old),
            frame(f.scope, &id, 1, audio(0, 1)),
        ])))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        stream.message().await.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    assert!(f.device.submissions.lock().unwrap().is_empty());
    drop(stream);
    f.finish().await;
}

#[tokio::test]
async fn heartbeats_cannot_extend_absolute_observation_deadline() {
    let mut f = Fixture::new(8).await;
    let id = Uuid::new_v4().to_string();
    let mut stream = f
        .client
        .push(authenticated(tokio_stream::iter([
            frame(f.scope, &id, 1, start()),
            frame(f.scope, &id, 1, audio(0, 1)),
            frame(f.scope, &id, 1, end(1)),
        ])))
        .await
        .unwrap()
        .into_inner();
    let accepted = until(&mut stream, |r| r.end_accepted).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(60)).await;
    // Resume real I/O before waiting on SQLite/network. No device-clock event is emitted.
    tokio::time::resume();
    let heartbeat = receipt_from(&mut stream).await;
    assert_eq!(heartbeat.observation_end, 0);
    assert!(heartbeat.receipt_sequence > accepted.receipt_sequence);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();
    let retired = until(&mut stream, |r| r.observation_end != 0).await;
    assert_eq!(
        retired.observation_end,
        proto::ObservationEnd::Cancelled as i32
    );
    assert_eq!(retired.submitted_samples, Some(0));
    assert!(f.device.state.lock().unwrap().1.is_empty());
    drop(stream);
    f.finish().await;
}

#[tokio::test]
async fn end_ack_waits_until_input_lease_is_released() {
    let mut f = Fixture::new(20).await;
    f.device.hold_end.store(true, Ordering::Release);
    let id = Uuid::new_v4().to_string();
    let mut stream = f
        .client
        .push(authenticated(tokio_stream::iter([
            frame(f.scope, &id, 1, start()),
            frame(f.scope, &id, 1, audio(0, 1)),
            frame(f.scope, &id, 1, end(1)),
        ])))
        .await
        .unwrap()
        .into_inner();
    until(&mut stream, |r| r.accepted_samples == 1).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !f.device.end_entered.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let pending_end_heartbeat = receipt_from(&mut stream).await;
    assert!(
        !pending_end_heartbeat.end_accepted,
        "pending backend End is not an input lease release ACK"
    );
    assert_eq!(pending_end_heartbeat.observation_end, 0);
    f.device.hold_end.store(false, Ordering::Release);
    f.device.changed.notify_all();
    until(&mut stream, |r| r.end_accepted).await;
    let next_id = Uuid::new_v4().to_string();
    let mut next = f
        .client
        .push(authenticated(tokio_stream::iter([
            frame(f.scope, &next_id, 2, start()),
            frame(f.scope, &next_id, 2, end(0)),
        ])))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(receipt_from(&mut next).await.accepted_samples, 0);
    drop(next);
    drop(stream);
    f.finish().await;
}

#[tokio::test]
async fn revocation_while_waiting_for_start_cannot_advance_playback_epoch() {
    let mut f = Fixture::new(8).await;
    let (tx, rx) = mpsc::channel(2);
    let mut stream = f
        .client
        .push(authenticated(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
        .await
        .unwrap()
        .into_inner();
    f.binding.unbind("server").await.unwrap();
    let newer = VoiceScope {
        generation_epoch: 2,
        ..f.scope
    };
    tx.send(frame(newer, &Uuid::new_v4().to_string(), 1, start()))
        .await
        .unwrap();
    drop(tx);
    assert_eq!(
        stream.message().await.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    assert!(f.device.submissions.lock().unwrap().is_empty());
    assert_eq!(f.device.state.lock().unwrap().0, 2);
    assert_eq!(f.session.active_scope().unwrap(), None);
    drop(stream);
    f.finish().await;
}

#[tokio::test]
async fn precise_fence_ack_waits_for_final_inflight_submission_snapshot() {
    let mut f = Fixture::new(8).await;
    let id = Uuid::new_v4().to_string();
    let mut stream = f
        .client
        .push(authenticated(tokio_stream::iter([
            frame(f.scope, &id, 1, start()),
            frame(f.scope, &id, 1, audio(0, 1)),
            frame(f.scope, &id, 1, end(1)),
        ])))
        .await
        .unwrap()
        .into_inner();
    let accepted = until(&mut stream, |r| r.end_accepted).await;
    let submission = f
        .device
        .submissions
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    let write = submission.begin_output().unwrap();
    let mut client = f.client.clone();
    let fence_id = id.clone();
    let scope = f.scope;
    let fence = tokio::spawn(async move {
        client
            .fence(authenticated(proto::FenceRequest {
                scope: Some(encode_scope(scope)),
                playback_id: Some(fence_id),
                playback_sequence: Some(1),
                close_session: false,
                reason: "settle entered output".into(),
            }))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while submission.is_live() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!fence.is_finished());
    assert_eq!(submission.snapshot(100).observation_end, None);
    write.finish(1);
    let reply = fence.await.unwrap().unwrap().into_inner();
    assert!(reply.applied);
    let final_progress = reply.final_progress.unwrap();
    assert_eq!(final_progress.submitted_samples, Some(1));
    assert_eq!(
        final_progress.observation_end,
        proto::ObservationEnd::Cancelled as i32
    );
    assert!(final_progress.receipt_sequence > accepted.receipt_sequence);
    assert_eq!(final_progress.playback_id, id);
    assert_eq!(final_progress.presented_samples, None);
    drop(stream);
    f.finish().await;
}

#[tokio::test]
async fn precise_fence_times_out_without_claiming_native_quiescence() {
    let mut f = Fixture::new(8).await;
    let id = Uuid::new_v4().to_string();
    let mut stream = f
        .client
        .push(authenticated(tokio_stream::iter([
            frame(f.scope, &id, 1, start()),
            frame(f.scope, &id, 1, audio(0, 1)),
            frame(f.scope, &id, 1, end(1)),
        ])))
        .await
        .unwrap()
        .into_inner();
    until(&mut stream, |r| r.end_accepted).await;
    let submission = f
        .device
        .submissions
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    let write = submission.begin_output().unwrap();
    let reply = tokio::time::timeout(
        Duration::from_secs(3),
        f.client.fence(authenticated(proto::FenceRequest {
            scope: Some(encode_scope(f.scope)),
            playback_id: Some(id),
            playback_sequence: Some(1),
            close_session: false,
            reason: "hung output".into(),
        })),
    )
    .await
    .unwrap();
    assert!(
        reply.is_err(),
        "admission fence alone cannot acknowledge precise cancellation"
    );
    assert!(!submission.output_quiescent());
    assert_eq!(submission.snapshot(100).observation_end, None);
    write.finish(1);
    let terminal = until(&mut stream, |r| r.observation_end != 0).await;
    assert_eq!(terminal.submitted_samples, Some(1));
    drop(stream);
    f.finish().await;
}

mod control;
