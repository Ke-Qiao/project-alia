use super::*;

#[derive(Default)]
struct TestObserver;
#[async_trait::async_trait]
impl PlaybackObserver for TestObserver {
    async fn start(&self, _: u32, _: u16) -> Result<()> {
        Ok(())
    }
    async fn offered(&self, _: u64) -> Result<()> {
        Ok(())
    }
    async fn progress(&self, _: PlaybackProgress) -> Result<()> {
        Ok(())
    }
}
fn identity(scope: VoiceScope, sequence: u64) -> PlaybackIdentity {
    PlaybackIdentity {
        scope,
        playback_id: uuid::Uuid::new_v4(),
        playback_sequence: sequence,
    }
}

fn wav(rate: u32, pcm: &[i16], declared: bool) -> Vec<u8> {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + pcm.len() as u32 * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(if declared { pcm.len() as u32 * 2 } else { 0 }).to_le_bytes());
    bytes.extend(pcm.iter().flat_map(|sample| sample.to_le_bytes()));
    bytes
}

#[tokio::test]
async fn fragmented_wav_preserves_original_scope_sequence_and_actual_rate() {
    let scope = VoiceScope {
        device_boot_id: uuid::Uuid::new_v4(),
        session_id: uuid::Uuid::new_v4(),
        generation_epoch: 7,
    };
    let source = wav(16000, &[1, 2, 3, 4], true);
    let payloads: Vec<_> = source
        .chunks(7)
        .map(|data| {
            Ok(AudioPayload {
                data: data.to_vec(),
                sample_rate: 24000,
                channels: 1,
                format: AudioFormat::Wav,
            })
        })
        .collect();
    let (tx, mut rx) = mpsc::channel(2);
    let (ack, ready) = oneshot::channel();
    let mut ack = Some(ack);
    let identity = identity(scope, 31);
    let producer = tokio::spawn(produce(
        Box::pin(futures::stream::iter(payloads)),
        tx,
        identity,
        Arc::new(AtomicU64::new(0)),
        Arc::new(TestObserver),
        ready,
        CancellationToken::new(),
        Arc::new(AtomicBool::new(false)),
    ));
    let mut samples = Vec::new();
    let mut next_sequence = 0;
    while let Some(frame) = rx.recv().await {
        assert_eq!(frame.scope, Some(encode_scope(scope)));
        assert_eq!(frame.playback_sequence, 31);
        assert_eq!(frame.playback_id, identity.playback_id.to_string());
        match frame.payload.unwrap() {
            playback_frame::Payload::Start(start) => {
                assert_eq!((start.sample_rate_hz, start.channels), (16000, 1));
                assert_eq!(start.contract_version, PLAYBACK_CONTRACT_VERSION);
                ack.take().unwrap().send(()).unwrap();
            }
            playback_frame::Payload::Audio(audio) => {
                assert_eq!(audio.sequence, next_sequence);
                next_sequence += 1;
                assert_eq!(audio.sample_offset, samples.len() as u64);
                samples.extend(
                    audio
                        .pcm_s16le
                        .chunks_exact(2)
                        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]])),
                );
            }
            playback_frame::Payload::End(end) => assert_eq!(end.total_samples, 4),
        }
    }
    assert_eq!(samples, [1, 2, 3, 4]);
    assert_eq!(producer.await.unwrap().unwrap(), 4);
}

#[test]
fn decoder_rejects_truncation_and_format_changes_without_speaking_headers() {
    let mut decoder = PcmDecoder::default();
    let bytes = wav(32000, &[1, 2], false);
    let mut pcm = Vec::new();
    for byte in bytes {
        pcm.extend(
            decoder
                .push(AudioPayload {
                    data: vec![byte],
                    sample_rate: 24000,
                    channels: 1,
                    format: AudioFormat::Wav,
                })
                .unwrap(),
        );
    }
    decoder.finish().unwrap();
    assert_eq!(pcm, vec![1, 0, 2, 0]);
    assert_eq!(decoder.spec, Some((32000, 1)));
    assert!(
        decoder
            .push(AudioPayload {
                data: vec![0, 0],
                sample_rate: 32000,
                channels: 1,
                format: AudioFormat::Pcm16
            })
            .is_err()
    );
    let mut truncated = PcmDecoder::default();
    let mut bytes = wav(16000, &[1, 2], true);
    bytes.pop();
    truncated
        .push(AudioPayload {
            data: bytes,
            sample_rate: 16000,
            channels: 1,
            format: AudioFormat::Wav,
        })
        .unwrap();
    assert!(truncated.finish().is_err());
}

#[test]
fn fence_receipt_must_prove_matching_session_and_minimum_epoch() {
    let scope = VoiceScope {
        device_boot_id: uuid::Uuid::new_v4(),
        session_id: uuid::Uuid::new_v4(),
        generation_epoch: 3,
    };
    let mut receipt = proto::FenceReceipt {
        final_progress: None,
        playback_id: None,
        playback_sequence: None,
        applied: false,
        active_scope: None,
        session_closed: true,
    };
    assert!(validate_fence_receipt(scope, false, &receipt).is_err());
    assert!(validate_fence_receipt(scope, true, &receipt).is_ok());
    receipt.session_closed = false;
    receipt.active_scope = Some(encode_scope(VoiceScope {
        generation_epoch: 2,
        ..scope
    }));
    assert!(validate_fence_receipt(scope, false, &receipt).is_err());
    receipt.active_scope = Some(encode_scope(scope));
    assert!(
        validate_fence_receipt(scope, false, &receipt).is_ok(),
        "idempotent applied=false can satisfy the floor"
    );
    assert!(validate_fence_receipt(scope, true, &receipt).is_err());
    receipt.active_scope.as_mut().unwrap().session_id = uuid::Uuid::new_v4().to_string();
    assert!(validate_fence_receipt(scope, false, &receipt).is_err());
    assert!(
        validate_fence_receipt(scope, true, &receipt).is_ok(),
        "late old close leaves a replacement session alone"
    );
}

#[tokio::test]
async fn idle_receipt_timeout_cancels_and_joins_the_audio_producer() {
    use std::pin::Pin;
    use std::sync::atomic::AtomicBool;
    use tonic::{Request, Response, Status};

    struct SilentDevice;
    #[tonic::async_trait]
    impl proto::playback_server::Playback for SilentDevice {
        type PushStream =
            Pin<Box<dyn futures::Stream<Item = Result<proto::PlaybackReceipt, Status>> + Send>>;
        async fn push(
            &self,
            request: Request<tonic::Streaming<proto::PlaybackFrame>>,
        ) -> Result<Response<Self::PushStream>, Status> {
            let pending = async_stream::stream! {
                let _incoming = request.into_inner();
                std::future::pending::<()>().await;
                yield Ok(proto::PlaybackReceipt::default());
            };
            Ok(Response::new(Box::pin(pending)))
        }
        async fn fence(
            &self,
            _: Request<proto::FenceRequest>,
        ) -> Result<Response<proto::FenceReceipt>, Status> {
            Err(Status::unimplemented("unused"))
        }
    }
    struct SourceDropped(Arc<AtomicBool>);
    impl Drop for SourceDropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let shutdown = tokio_util::sync::CancellationToken::new();
    let stop = shutdown.clone();
    let server = tokio::spawn(async move {
        let incoming = async_stream::stream! { loop { yield listener.accept().await.map(|(stream, _)| stream); } };
        tonic::transport::Server::builder()
            .add_service(proto::playback_server::PlaybackServer::new(SilentDevice))
            .serve_with_incoming_shutdown(incoming, stop.cancelled())
            .await
            .unwrap();
    });
    let client = PlaybackClient::connect(endpoint).await.unwrap();
    let dropped = Arc::new(AtomicBool::new(false));
    let watched = dropped.clone();
    let audio = async_stream::stream! {
        let _dropped = SourceDropped(watched);
        yield Ok(AudioPayload { data: vec![1, 0], sample_rate: 16000, channels: 1, format: AudioFormat::Pcm16 });
        std::future::pending::<()>().await;
    };
    let scope = VoiceScope {
        device_boot_id: uuid::Uuid::new_v4(),
        session_id: uuid::Uuid::new_v4(),
        generation_epoch: 1,
    };
    let report = push(
        client,
        identity(scope, 1),
        Box::pin(audio),
        tonic::metadata::MetadataMap::new(),
        Arc::new(TestObserver),
        CancellationToken::new(),
        Duration::from_millis(50),
    )
    .await
    .unwrap();
    assert!(
        matches!(report.observation_end, ObservationEnd::Unknown { reason } if reason.contains("timeout"))
    );
    assert!(
        dropped.load(Ordering::Acquire),
        "producer must be joined after cancellation"
    );
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}

#[derive(Default)]
struct EvidenceObserver {
    format: std::sync::Mutex<Option<(u32, u16)>>,
    offered: AtomicU64,
    progress: std::sync::Mutex<Vec<PlaybackProgress>>,
    changed: tokio::sync::Notify,
    start_gate: Option<Arc<tokio::sync::Semaphore>>,
    offer_gate: Option<Arc<tokio::sync::Semaphore>>,
}
#[async_trait::async_trait]
impl PlaybackObserver for EvidenceObserver {
    async fn start(&self, rate: u32, channels: u16) -> Result<()> {
        *self.format.lock().unwrap() = Some((rate, channels));
        self.changed.notify_one();
        if let Some(gate) = &self.start_gate {
            gate.acquire().await?.forget();
        }
        Ok(())
    }
    async fn offered(&self, frames: u64) -> Result<()> {
        self.changed.notify_one();
        if let Some(gate) = &self.offer_gate {
            gate.acquire().await?.forget();
        }
        self.offered.store(frames, Ordering::Release);
        Ok(())
    }
    async fn progress(&self, progress: PlaybackProgress) -> Result<()> {
        self.progress.lock().unwrap().push(progress);
        self.changed.notify_one();
        Ok(())
    }
}
impl EvidenceObserver {
    async fn wait(&self, condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let changed = self.changed.notified();
                if condition() {
                    return;
                }
                changed.await;
            }
        })
        .await
        .unwrap();
    }
}

#[derive(Clone, Copy)]
enum FenceProof {
    Latest,
    Missing,
    Submitted(u64),
}
struct EvidenceDevice {
    proof: FenceProof,
    observer: Arc<EvidenceObserver>,
    wire: Arc<std::sync::Mutex<Vec<proto::PlaybackFrame>>>,
    commands: std::sync::Mutex<Option<mpsc::UnboundedReceiver<Option<u64>>>>,
    fences: Arc<std::sync::Mutex<Vec<proto::FenceRequest>>>,
}
#[tonic::async_trait]
impl proto::playback_server::Playback for EvidenceDevice {
    type PushStream = std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<proto::PlaybackReceipt, tonic::Status>> + Send>,
    >;
    async fn push(
        &self,
        request: tonic::Request<tonic::Streaming<proto::PlaybackFrame>>,
    ) -> Result<tonic::Response<Self::PushStream>, tonic::Status> {
        let mut commands = self.commands.lock().unwrap().take().unwrap();
        let mut incoming = request.into_inner();
        let wire = self.wire.clone();
        let observer = self.observer.clone();
        let stream = async_stream::stream! {
            let first = incoming.message().await.unwrap().unwrap();
            assert_eq!(*observer.format.lock().unwrap(), Some((16000, 1)));
            wire.lock().unwrap().push(first.clone());
            let mut receipt = proto::PlaybackReceipt {
                scope: first.scope.clone(), playback_id: first.playback_id.clone(), playback_sequence: first.playback_sequence,
                state: proto::PlaybackState::Accepted as i32, contract_version: PLAYBACK_CONTRACT_VERSION,
                receipt_sequence: 1, submitted_samples: Some(0),
                backend: Some(proto::SubmissionBackend { instance_id: uuid::Uuid::new_v4().to_string(),
                    boundary: proto::SubmissionBoundary::AlsaWriteAccepted as i32, output_sample_rate_hz: 48000, output_channels: 2 }),
                ..Default::default()
            };
            yield Ok(receipt.clone());
            let mut ended = false;
            while let Some(frame) = incoming.message().await.unwrap() {
                wire.lock().unwrap().push(frame.clone());
                match frame.payload.unwrap() {
                    playback_frame::Payload::Audio(audio) => {
                        receipt.accepted_samples += audio.pcm_s16le.len() as u64 / 2;
                        assert!(observer.offered.load(Ordering::Acquire) >= receipt.accepted_samples,
                            "Audio must follow durable offered admission");
                    },
                    playback_frame::Payload::End(end) => { assert_eq!(end.total_samples, receipt.accepted_samples); ended = true; },
                    _ => panic!("duplicate Start"),
                }
                if !ended {
                    receipt.receipt_sequence += 1;
                    yield Ok(receipt.clone());
                }
            }
            assert!(ended);
            receipt.end_accepted = true;
            receipt.receipt_sequence += 1;
            yield Ok(receipt.clone());
            while let Some(submitted) = commands.recv().await {
                receipt.receipt_sequence += 1;
                if let Some(submitted) = submitted { receipt.submitted_samples = Some(submitted); }
                if receipt.submitted_samples == Some(receipt.accepted_samples) {
                    receipt.observation_end = proto::ObservationEnd::SubmissionFinished as i32;
                    receipt.state = proto::PlaybackState::Submitted as i32;
                }
                yield Ok(receipt.clone());
                if receipt.observation_end != 0 { break; }
            }
        };
        Ok(tonic::Response::new(Box::pin(stream)))
    }
    async fn fence(
        &self,
        request: tonic::Request<proto::FenceRequest>,
    ) -> Result<tonic::Response<proto::FenceReceipt>, tonic::Status> {
        let request = request.into_inner();
        self.fences.lock().unwrap().push(request.clone());
        let mut final_progress =
            self.observer
                .progress
                .lock()
                .unwrap()
                .last()
                .map(|last| proto::PlaybackReceipt {
                    scope: Some(encode_scope(last.identity.scope)),
                    playback_id: last.identity.playback_id.to_string(),
                    playback_sequence: last.identity.playback_sequence,
                    receipt_sequence: last.receipt_sequence + 1,
                    contract_version: PLAYBACK_CONTRACT_VERSION,
                    accepted_samples: last.accepted_source_frames,
                    submitted_samples: last.submitted_source_frames,
                    end_accepted: last.end_accepted,
                    observation_end: proto::ObservationEnd::Cancelled as i32,
                    state: proto::PlaybackState::Cancelled as i32,
                    reason: request.reason.clone(),
                    backend: last.backend.as_ref().map(|b| proto::SubmissionBackend {
                        instance_id: b.instance_id.to_string(),
                        boundary: proto::SubmissionBoundary::AlsaWriteAccepted as i32,
                        output_sample_rate_hz: b.output_sample_rate_hz,
                        output_channels: b.output_channels,
                    }),
                    ..Default::default()
                });
        match self.proof {
            FenceProof::Latest => {}
            FenceProof::Missing => final_progress = None,
            FenceProof::Submitted(frames) => {
                final_progress.as_mut().unwrap().submitted_samples = Some(frames)
            }
        }
        Ok(tonic::Response::new(proto::FenceReceipt {
            applied: true,
            final_progress,
            active_scope: request.scope,
            session_closed: false,
            playback_id: request.playback_id,
            playback_sequence: request.playback_sequence,
        }))
    }
}
struct EvidenceFixture {
    client: PlaybackClient<Channel>,
    commands: mpsc::UnboundedSender<Option<u64>>,
    wire: Arc<std::sync::Mutex<Vec<proto::PlaybackFrame>>>,
    fences: Arc<std::sync::Mutex<Vec<proto::FenceRequest>>>,
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}
impl EvidenceFixture {
    async fn new(observer: Arc<EvidenceObserver>) -> Self {
        Self::with_proof(observer, FenceProof::Latest).await
    }
    async fn with_proof(observer: Arc<EvidenceObserver>, proof: FenceProof) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let wire = Arc::new(std::sync::Mutex::new(Vec::new()));
        let fences = Arc::new(std::sync::Mutex::new(Vec::new()));
        let device = EvidenceDevice {
            proof,
            observer,
            wire: wire.clone(),
            commands: std::sync::Mutex::new(Some(receiver)),
            fences: fences.clone(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        let server = tokio::spawn(async move {
            let incoming = async_stream::stream! { loop { yield listener.accept().await.map(|(stream, _)| stream); } };
            tonic::transport::Server::builder()
                .add_service(proto::playback_server::PlaybackServer::new(device))
                .serve_with_incoming_shutdown(incoming, stop.cancelled())
                .await
                .unwrap();
        });
        let client = PlaybackClient::connect(endpoint).await.unwrap();
        Self {
            client,
            commands,
            wire,
            fences,
            shutdown,
            server,
        }
    }
    async fn finish(self) {
        drop(self.client);
        drop(self.commands);
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.server)
            .await
            .unwrap()
            .unwrap();
    }
}
fn delivery_identity() -> PlaybackIdentity {
    identity(
        VoiceScope {
            device_boot_id: uuid::Uuid::new_v4(),
            session_id: uuid::Uuid::new_v4(),
            generation_epoch: 1,
        },
        5,
    )
}
fn stereo_audio() -> AudioPayloadStream {
    Box::pin(futures::stream::iter([Ok(AudioPayload {
        data: [1000_i16, 3000, -1000, 1000, 4000, 2000, -2000, -4000]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect(),
        sample_rate: 16000,
        channels: 2,
        format: AudioFormat::Pcm16,
    })]))
}

#[tokio::test]
async fn durable_gates_precede_wire_and_end_continues_monotonic_submission_evidence() {
    let start_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let offer_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let observer = Arc::new(EvidenceObserver {
        start_gate: Some(start_gate.clone()),
        offer_gate: Some(offer_gate.clone()),
        ..Default::default()
    });
    let f = EvidenceFixture::new(observer.clone()).await;
    let id = delivery_identity();
    let delivery = tokio::spawn(push(
        f.client.clone(),
        id,
        stereo_audio(),
        Default::default(),
        observer.clone(),
        CancellationToken::new(),
        Duration::from_secs(5),
    ));
    observer
        .wait(|| observer.format.lock().unwrap().is_some())
        .await;
    assert!(
        f.wire.lock().unwrap().is_empty(),
        "Start awaits durable format admission"
    );
    start_gate.add_permits(1);
    observer
        .wait(|| !observer.progress.lock().unwrap().is_empty())
        .await;
    assert_eq!(
        f.wire.lock().unwrap().len(),
        1,
        "Audio awaits durable offered admission"
    );
    offer_gate.add_permits(1);
    observer
        .wait(|| {
            observer
                .progress
                .lock()
                .unwrap()
                .last()
                .is_some_and(|p| p.end_accepted)
        })
        .await;
    assert!(
        !delivery.is_finished(),
        "EndAccepted is not a submission observation end"
    );
    f.commands.send(Some(2)).unwrap();
    observer
        .wait(|| {
            observer
                .progress
                .lock()
                .unwrap()
                .last()
                .is_some_and(|p| p.submitted_source_frames == Some(2))
        })
        .await;
    f.commands.send(Some(4)).unwrap();
    let report = delivery.await.unwrap().unwrap();
    assert_eq!(report.observation_end, ObservationEnd::SubmissionFinished);
    let progress = report.progress.unwrap();
    assert_eq!(progress.identity, id);
    assert_eq!(
        progress.accepted_source_frames, 4,
        "stereo counts frames, not scalar samples"
    );
    assert_eq!(progress.submitted_source_frames, Some(4));
    assert_eq!(progress.presented_source_frames, None);
    let observed = observer.progress.lock().unwrap().clone();
    for pair in observed.windows(2) {
        pair[1].validate(Some(&pair[0]), 4).unwrap();
    }
    let bytes: Vec<u8> = f
        .wire
        .lock()
        .unwrap()
        .iter()
        .flat_map(|f| match &f.payload {
            Some(playback_frame::Payload::Audio(audio)) => audio.pcm_s16le.clone(),
            _ => Vec::new(),
        })
        .collect();
    let pcm: Vec<_> = bytes
        .chunks_exact(2)
        .map(|s| i16::from_le_bytes([s[0], s[1]]))
        .collect();
    assert_eq!(pcm, [2000, 0, 3000, -3000]);
    assert!(f.fences.lock().unwrap().is_empty());
    f.finish().await;
}

#[tokio::test]
async fn heartbeats_do_not_extend_delivery_deadline_and_exact_cancel_keeps_prefix() {
    let observer = Arc::new(EvidenceObserver::default());
    let f = EvidenceFixture::new(observer.clone()).await;
    let id = delivery_identity();
    let delivery = tokio::spawn(push(
        f.client.clone(),
        id,
        stereo_audio(),
        Default::default(),
        observer.clone(),
        CancellationToken::new(),
        Duration::from_secs(5),
    ));
    observer
        .wait(|| {
            observer
                .progress
                .lock()
                .unwrap()
                .last()
                .is_some_and(|p| p.end_accepted)
        })
        .await;
    f.commands.send(Some(2)).unwrap();
    observer
        .wait(|| {
            observer
                .progress
                .lock()
                .unwrap()
                .last()
                .is_some_and(|p| p.submitted_source_frames == Some(2))
        })
        .await;
    // Keep the receipt-idle deadline alive while advancing virtual wall time.
    for _ in 0..119 {
        let before = observer.progress.lock().unwrap().len();
        f.commands.send(None).unwrap();
        observer
            .wait(|| observer.progress.lock().unwrap().len() > before)
            .await;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::time::resume();
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::time::resume();
    let report = tokio::time::timeout(Duration::from_secs(3), delivery)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(report.observation_end, ObservationEnd::Cancelled { reason } if reason.contains("absolute"))
    );
    assert_eq!(report.progress.unwrap().submitted_source_frames, Some(2));
    let fences = f.fences.lock().unwrap().clone();
    assert_eq!(fences.len(), 1);
    assert_eq!(fences[0].playback_id, Some(id.playback_id.to_string()));
    assert_eq!(fences[0].playback_sequence, Some(id.playback_sequence));
    assert!(!fences[0].close_session);
    f.finish().await;
}

#[test]
fn decoder_rejects_unimplemented_presentation_and_wrong_identity_or_contract() {
    let id = delivery_identity();
    let valid = proto::PlaybackReceipt {
        scope: Some(encode_scope(id.scope)),
        playback_id: id.playback_id.to_string(),
        playback_sequence: id.playback_sequence,
        contract_version: 2,
        receipt_sequence: 1,
        state: proto::PlaybackState::Accepted as i32,
        ..Default::default()
    };
    assert!(decode_receipt(id, valid.clone()).is_ok());
    let mut wrong = valid.clone();
    wrong.presented_samples = Some(0);
    assert!(decode_receipt(id, wrong).is_err());
    let mut wrong = valid.clone();
    wrong.playback_id = uuid::Uuid::new_v4().to_string();
    assert!(decode_receipt(id, wrong).is_err());
    let mut wrong = valid.clone();
    wrong.contract_version = 1;
    assert!(decode_receipt(id, wrong).is_err());
    let mut wrong = valid;
    wrong.observation_end = proto::ObservationEnd::PresentedFinished as i32;
    assert!(decode_receipt(id, wrong).is_err());
}

#[tokio::test]
async fn precise_cancel_merges_final_prefix_and_bare_control_ack_is_unknown() {
    for proof in [FenceProof::Submitted(3), FenceProof::Missing] {
        let observer = Arc::new(EvidenceObserver::default());
        let f = EvidenceFixture::with_proof(observer.clone(), proof).await;
        let id = delivery_identity();
        let cancel = CancellationToken::new();
        let delivery = tokio::spawn(push(
            f.client.clone(),
            id,
            stereo_audio(),
            Default::default(),
            observer.clone(),
            cancel.clone(),
            Duration::from_secs(5),
        ));
        observer
            .wait(|| {
                observer
                    .progress
                    .lock()
                    .unwrap()
                    .last()
                    .is_some_and(|p| p.end_accepted)
            })
            .await;
        f.commands.send(Some(2)).unwrap();
        observer
            .wait(|| {
                observer
                    .progress
                    .lock()
                    .unwrap()
                    .last()
                    .is_some_and(|p| p.submitted_source_frames == Some(2))
            })
            .await;
        let before_cancel = observer.progress.lock().unwrap().clone();
        cancel.cancel();
        let report = delivery.await.unwrap().unwrap();
        match proof {
            FenceProof::Submitted(_) => {
                assert!(matches!(
                    report.observation_end,
                    ObservationEnd::Cancelled { .. }
                ));
                assert_eq!(
                    report.progress.unwrap().submitted_source_frames,
                    Some(3),
                    "entered write settled after the last Push receipt"
                );
            }
            FenceProof::Missing => {
                assert_eq!(
                    *observer.progress.lock().unwrap(),
                    before_cancel,
                    "local Unknown must not invent a device receipt or occupy its sequence"
                );
                assert_eq!(report.progress.as_ref(), before_cancel.last());
                assert!(matches!(
                    report.observation_end,
                    ObservationEnd::Unknown { .. }
                ));
                assert_eq!(report.progress.unwrap().submitted_source_frames, Some(2));
            }
            _ => unreachable!(),
        }
        f.finish().await;
    }
}
