use super::*;
use crate::module::lifecycle::ModuleLifecycle;
use crate::module::traits::AudioPlaybackBackend;
use crate::storage::EmbeddedStore;
use std::sync::{Condvar, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct Device {
    state: Mutex<(u64, Vec<i16>)>,
    changed: Condvar,
    capacity: usize,
}
#[tonic::async_trait]
impl ModuleLifecycle for Device {
    fn name(&self) -> &'static str {
        "test-playback"
    }
}
impl AudioPlaybackBackend for Device {
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
        state.1.clear();
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
        state.1.clear();
        self.changed.notify_all();
        Ok(true)
    }
    fn is_playback_current(&self, generation: u64) -> bool {
        self.state.lock().unwrap().0 == generation
    }
}

struct Fixture {
    client: proto::playback_client::PlaybackClient<tonic::transport::Channel>,
    scope: VoiceScope,
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
        binding
            .seed_dev_binding("server", "server", "http://localhost:1234")
            .await
            .unwrap();
        let device = Arc::new(Device {
            state: Mutex::new((0, Vec::new())),
            changed: Condvar::new(),
            capacity,
        });
        let session = Arc::new(PlaybackSession::new(device.clone()));
        let scope = VoiceScope {
            device_boot_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            generation_epoch: 1,
        };
        session.activate(&scope).unwrap();
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
        let client = proto::playback_client::PlaybackClient::connect(endpoint)
            .await
            .unwrap();
        Self {
            client,
            scope,
            session,
            device,
            binding,
            shutdown,
            server,
            _dir: dir,
        }
    }

    async fn finish(self) {
        self.session.close(&self.scope).unwrap();
        drop(self.client);
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.server)
            .await
            .unwrap()
            .unwrap();
    }
}

fn authenticated<T>(body: T) -> Request<T> {
    let mut request = Request::new(body);
    request
        .metadata_mut()
        .insert("authorization", "Bearer test-dev-binding".parse().unwrap());
    request
}

fn frame(
    scope: VoiceScope,
    id: &str,
    playback_sequence: u64,
    payload: playback_frame::Payload,
) -> proto::PlaybackFrame {
    proto::PlaybackFrame {
        scope: Some(encode_scope(scope)),
        playback_id: id.into(),
        playback_sequence,
        payload: Some(payload),
    }
}
fn start() -> playback_frame::Payload {
    playback_frame::Payload::Start(proto::PlaybackStart {
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
    assert!(receipt.submitted_samples.is_none() && receipt.presented_samples.is_none());
    assert_ne!(receipt.state, proto::PlaybackState::Completed as i32);
    receipt
}

#[tokio::test]
async fn grpc_fence_unblocks_full_media_queue_and_old_fence_cannot_stop_new_epoch() {
    let mut f = Fixture::new(1).await;
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel(4);
    tx.send(frame(f.scope, &id, 0, start())).await.unwrap();
    tx.send(frame(f.scope, &id, 0, audio(0, 1))).await.unwrap();
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
    tx.send(frame(f.scope, &id, 0, audio(1, 2))).await.unwrap();
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
    let frames = [
        frame(newer, &next_id, 1, start()),
        frame(newer, &next_id, 1, audio(0, 3)),
        frame(newer, &next_id, 1, end(1)),
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
    assert!(next.message().await.unwrap().is_none());
    let stale = f
        .client
        .fence(authenticated(proto::FenceRequest {
            scope: Some(encode_scope(f.scope)),
            reason: "late old interrupt".into(),
            close_session: false,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!stale.applied);
    assert_eq!(f.device.state.lock().unwrap().1, vec![3]);
    drop(next);
    f.finish().await;
}

#[tokio::test]
async fn grpc_rejects_media_after_end_and_revocation_retires_queued_audio() {
    let mut f = Fixture::new(10).await;
    let id = Uuid::new_v4().to_string();
    let frames = [
        frame(f.scope, &id, 0, start()),
        frame(f.scope, &id, 0, audio(0, 1)),
        frame(f.scope, &id, 0, end(1)),
        frame(f.scope, &id, 0, audio(1, 2)),
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
    tx.send(frame(f.scope, &id, 1, start())).await.unwrap();
    tx.send(frame(f.scope, &id, 1, audio(0, 3))).await.unwrap();
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
    f.binding.unbind("server").await.unwrap();
    assert_eq!(
        receipt_from(&mut stream).await.state,
        proto::PlaybackState::Cancelled as i32
    );
    assert!(f.device.state.lock().unwrap().1.is_empty());
    drop(stream);
    drop(tx);
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
