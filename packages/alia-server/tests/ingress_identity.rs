#![cfg(feature = "channel-embedded")]
//! Real gRPC ingress checks: signed devices cannot select another account.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
};
use std::time::Duration;

use alia_proto::audio::{
    self, server_audio_ingress_client::ServerAudioIngressClient,
    server_audio_ingress_server::ServerAudioIngressServer,
};
use alia_server::channels::ChannelRegistry;
use alia_server::channels::adapter::{
    ChannelAction, ChannelAdapter, ChannelCapabilities, ChannelEvent, ChannelTarget,
};
use alia_server::event::{EventBus, ServerEvent};
use alia_server::grpc::audio_ingress::ServerAudioIngressService;
use alia_server::module::ModuleLifecycle;
use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::Request;
use uuid::Uuid;

const OWNER: Uuid = Uuid::from_u128(41);
const OTHER_USER: Uuid = Uuid::from_u128(42);
const DEVICE: &str = "identity-test-device";

struct Device {
    owner: Option<Uuid>,
    revoked: AtomicBool,
    account_disabled: AtomicBool,
    checks: AtomicUsize,
    privacy_epoch: AtomicI64,
    relationship_epoch: AtomicI64,
    source_admissions: AtomicUsize,
    pause_next_auth: AtomicBool,
    resume_auth: tokio::sync::Semaphore,
}
impl ModuleLifecycle for Device {
    fn name(&self) -> &'static str {
        "identity-test-device"
    }
}
#[async_trait]
impl ChannelAdapter for Device {
    async fn authenticated_source_epochs(
        &self,
    ) -> anyhow::Result<alia_server::modules::memory::source::SourceEpochs> {
        let epochs = alia_server::modules::memory::source::SourceEpochs {
            privacy_epoch: self.privacy_epoch.load(Ordering::SeqCst),
            relationship_epoch: self.relationship_epoch.load(Ordering::SeqCst),
        };
        self.source_admissions.fetch_add(1, Ordering::SeqCst);
        Ok(epochs)
    }

    fn channel_type(&self) -> &'static str {
        "embedded"
    }
    fn channel_id(&self) -> &str {
        DEVICE
    }
    fn owner_user_id(&self) -> Option<Uuid> {
        self.owner
    }
    async fn authenticated_owner(&self) -> anyhow::Result<Uuid> {
        anyhow::ensure!(
            !self.account_disabled.load(Ordering::SeqCst),
            "account disabled"
        );
        self.owner
            .ok_or_else(|| anyhow::anyhow!("identity unlinked"))
    }
    fn capabilities(&self) -> ChannelCapabilities {
        Default::default()
    }
    async fn send_message(&self, _: &ChannelTarget, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn execute_action(&self, action: ChannelAction) -> anyhow::Result<serde_json::Value> {
        anyhow::ensure!(
            action.name == "verify_audio_ingress_auth",
            "unexpected action"
        );
        anyhow::ensure!(!self.revoked.load(Ordering::SeqCst), "device revoked");
        alia_transport::verify_auth_token(
            action.params["token"].as_str().unwrap_or_default(),
            ed25519_dalek::SigningKey::from_bytes(&[11; 32])
                .verifying_key()
                .as_bytes(),
            DEVICE,
            alia_transport::EMBEDDED_INGRESS_SCOPE,
            alia_transport::DEFAULT_AUTH_MAX_SKEW,
        )?;
        self.checks.fetch_add(1, Ordering::SeqCst);
        if self.pause_next_auth.swap(false, Ordering::SeqCst) {
            self.resume_auth.acquire().await.unwrap().forget();
        }
        Ok(serde_json::json!({"authenticated": true}))
    }
}

struct Fixture {
    client: ServerAudioIngressClient<tonic::transport::Channel>,
    device: Arc<Device>,
    events: broadcast::Receiver<ServerEvent>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
}
impl Fixture {
    async fn new(owner: Option<Uuid>) -> Self {
        let device = Arc::new(Device {
            owner,
            revoked: false.into(),
            account_disabled: false.into(),
            checks: 0.into(),
            privacy_epoch: 0.into(),
            relationship_epoch: 0.into(),
            source_admissions: 0.into(),
            pause_next_auth: false.into(),
            resume_auth: tokio::sync::Semaphore::new(0),
        });
        let channels = Arc::new(ChannelRegistry::new());
        channels.register(device.clone()).await.unwrap();
        let bus = EventBus::new(8);
        let events = bus.subscribe();
        let service = ServerAudioIngressService::new(bus, channels, DEVICE);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let incoming = async_stream::stream! {
            loop { yield listener.accept().await.map(|(socket, _)| socket); }
        };
        let cancel = CancellationToken::new();
        let shutdown = cancel.clone();
        let task = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(ServerAudioIngressServer::new(service))
                .serve_with_incoming_shutdown(incoming, shutdown.cancelled_owned())
                .await
        });
        let client = ServerAudioIngressClient::connect(endpoint).await.unwrap();
        Self {
            client,
            device,
            events,
            cancel,
            task,
        }
    }
    async fn shutdown(self) {
        self.cancel.cancel();
        drop(self.client);
        tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
fn request<T>(body: T) -> Request<T> {
    let token = alia_transport::mint_auth_token(
        &ed25519_dalek::SigningKey::from_bytes(&[11; 32]),
        DEVICE,
        alia_transport::EMBEDDED_INGRESS_SCOPE,
        Duration::from_secs(120),
    )
    .unwrap();
    let mut request = Request::new(body);
    request
        .metadata_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
        .metadata_mut()
        .insert("x-alia-channel-id", DEVICE.parse().unwrap());
    request
}
fn interrupt() -> audio::InterruptActiveTurnRequest {
    audio::InterruptActiveTurnRequest {
        channel_id: DEVICE.into(),
        user_id: OTHER_USER.to_string(),
        reason: "wakeword".into(),
        timestamp_us: 0,
    }
}
fn frame() -> audio::CapturedAudioChunk {
    audio::CapturedAudioChunk {
        chunk: Some(audio::AudioChunk {
            opus_data: vec![0; 3200],
            sequence: 0,
            timestamp_us: 0,
            encoding: audio::AudioEncoding::PcmS16le as i32,
        }),
        channel_id: DEVICE.into(),
        user_id: OTHER_USER.to_string(),
        sample_rate: 16_000,
        channels: 1,
    }
}

#[tokio::test]
async fn signed_unlinked_device_cannot_enter_conversation() {
    let mut f = Fixture::new(None).await;
    let error = f
        .client
        .interrupt_active_turn(request(interrupt()))
        .await
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    assert!(error.message().contains("identity_unlinked"));
    let error = f
        .client
        .upload_captured_audio(request(tokio_stream::iter([frame()])))
        .await
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    assert!(f.events.try_recv().is_err());
    f.shutdown().await;
}

#[tokio::test]
async fn active_device_cannot_enter_conversation_after_owner_is_disabled() {
    let mut f = Fixture::new(Some(OWNER)).await;
    f.device.account_disabled.store(true, Ordering::SeqCst);
    let error = f
        .client
        .upload_captured_audio(request(tokio_stream::iter([frame()])))
        .await
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::PermissionDenied);
    assert!(f.events.try_recv().is_err());
    f.shutdown().await;
}

#[tokio::test]
async fn interrupt_uses_configured_owner_despite_device_account_claim() {
    let mut f = Fixture::new(Some(OWNER)).await;
    assert!(
        f.client
            .interrupt_active_turn(request(interrupt()))
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    let ServerEvent::ChannelInput(ChannelEvent::InterruptActiveTurn { user_id, .. }) =
        f.events.recv().await.unwrap()
    else {
        panic!("expected interruption");
    };
    assert_eq!(user_id, OWNER.to_string());
    f.shutdown().await;
}

#[tokio::test]
async fn uploaded_audio_uses_configured_owner_despite_device_account_claim() {
    let mut f = Fixture::new(Some(OWNER)).await;
    assert!(
        f.client
            .upload_captured_audio(request(tokio_stream::iter([frame()])))
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    let ServerEvent::ChannelInput(ChannelEvent::AudioInput { user_id, audio, .. }) =
        f.events.recv().await.unwrap()
    else {
        panic!("expected audio input");
    };
    assert_eq!(user_id, OWNER.to_string());
    assert_eq!(audio.data.len(), 3200);
    assert_eq!(f.device.checks.load(Ordering::SeqCst), 2);
    f.shutdown().await;
}

#[tokio::test]
async fn revocation_while_upload_is_open_prevents_publication() {
    let mut f = Fixture::new(Some(OWNER)).await;
    let (tx, rx) = mpsc::channel(2);
    let mut client = f.client.clone();
    let upload = tokio::spawn(async move {
        client
            .upload_captured_audio(request(ReceiverStream::new(rx)))
            .await
    });
    tx.send(frame()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.device.checks.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.device.revoked.store(true, Ordering::SeqCst);
    drop(tx);
    let error = tokio::time::timeout(Duration::from_secs(2), upload)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::Unauthenticated);
    assert!(f.events.try_recv().is_err());
    f.shutdown().await;
}

#[tokio::test]
async fn upload_source_time_precedes_auth_wait_and_ignores_device_clock() {
    let mut f = Fixture::new(Some(OWNER)).await;
    f.device.pause_next_auth.store(true, Ordering::SeqCst);
    let mut forged = frame();
    forged.chunk.as_mut().unwrap().timestamp_us = u64::MAX;
    let received_after = chrono::Utc::now();
    let mut client = f.client.clone();
    let upload = tokio::spawn(async move {
        client
            .upload_captured_audio(request(tokio_stream::iter([forged])))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.device.checks.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let auth_wait_started = chrono::Utc::now();
    f.device.resume_auth.add_permits(1);
    assert!(upload.await.unwrap().unwrap().into_inner().accepted);
    let ServerEvent::ChannelInput(ChannelEvent::AudioInput {
        source_received_at, ..
    }) = f.events.recv().await.unwrap()
    else {
        panic!("expected audio input");
    };
    assert!(source_received_at >= received_after);
    assert!(source_received_at <= auth_wait_started);
    f.shutdown().await;
}

#[tokio::test]
async fn privacy_change_after_admission_before_media_rejects_old_upload() {
    changed_epoch_rejects_old_upload(true).await;
}

#[tokio::test]
async fn relationship_change_after_admission_before_media_rejects_old_upload() {
    changed_epoch_rejects_old_upload(false).await;
}

async fn changed_epoch_rejects_old_upload(privacy: bool) {
    let mut f = Fixture::new(Some(OWNER)).await;
    let (tx, rx) = mpsc::channel(2);
    let mut client = f.client.clone();
    let upload = tokio::spawn(async move {
        client
            .upload_captured_audio(request(ReceiverStream::new(rx)))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.device.source_admissions.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // No application media was read before admission. Media sent after a clear
    // still belongs to the already admitted request, not to a fresh source.
    let (changed, domain) = if privacy {
        (&f.device.privacy_epoch, "privacy")
    } else {
        (&f.device.relationship_epoch, "relationship")
    };
    changed.store(1, Ordering::SeqCst);
    tx.send(frame()).await.unwrap();
    drop(tx);
    let error = tokio::time::timeout(Duration::from_secs(2), upload)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::PermissionDenied);
    assert!(error.message().contains(&format!("{domain} epoch changed")));
    assert!(f.events.try_recv().is_err());

    f.client
        .upload_captured_audio(request(tokio_stream::iter([frame()])))
        .await
        .unwrap();
    let ServerEvent::ChannelInput(ChannelEvent::AudioInput {
        source_privacy_epoch,
        source_relationship_epoch,
        ..
    }) = f.events.recv().await.unwrap()
    else {
        panic!("expected fresh audio source")
    };
    assert_eq!(source_privacy_epoch, i64::from(privacy));
    assert_eq!(source_relationship_epoch, i64::from(!privacy));
    f.shutdown().await;
}

#[tokio::test]
async fn published_source_keeps_admitted_epoch_while_queued() {
    let mut f = Fixture::new(Some(OWNER)).await;
    f.device.privacy_epoch.store(12, Ordering::SeqCst);
    f.device.relationship_epoch.store(21, Ordering::SeqCst);
    f.client
        .upload_captured_audio(request(tokio_stream::iter([frame()])))
        .await
        .unwrap();
    f.device.privacy_epoch.store(13, Ordering::SeqCst);
    f.device.relationship_epoch.store(22, Ordering::SeqCst);
    let ServerEvent::ChannelInput(ChannelEvent::AudioInput {
        source_privacy_epoch,
        source_relationship_epoch,
        ..
    }) = f.events.recv().await.unwrap()
    else {
        panic!("expected queued audio source")
    };
    assert_eq!(source_privacy_epoch, 12);
    assert_eq!(source_relationship_epoch, 21);
    f.shutdown().await;
}
