use super::*;

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
    let producer = tokio::spawn(produce(
        Box::pin(futures::stream::iter(payloads)),
        tx,
        scope,
        "test-playback".into(),
        31,
        Arc::new(AtomicU64::new(0)),
    ));
    let mut samples = Vec::new();
    let mut next_sequence = 0;
    while let Some(frame) = rx.recv().await {
        assert_eq!(frame.scope, Some(encode_scope(scope)));
        assert_eq!(frame.playback_sequence, 31);
        match frame.payload.unwrap() {
            playback_frame::Payload::Start(start) => {
                assert_eq!((start.sample_rate_hz, start.channels), (16000, 1))
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
    let error = push(
        client,
        scope,
        0,
        Box::pin(audio),
        tonic::metadata::MetadataMap::new(),
        Duration::from_millis(50),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("timeout"));
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
