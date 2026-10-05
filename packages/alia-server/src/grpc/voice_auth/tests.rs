//! Authentication/source admission checks; transport coverage lives in voice_stream_live.
use super::*;
use crate::channels::adapter::{ChannelAction, ChannelCapabilities};
use crate::module::ModuleLifecycle;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::time::Duration;
use uuid::Uuid;
const OWNER: Uuid = Uuid::from_u128(41);
const DEVICE: &str = "identity-test-device";
struct Device {
    owner: Option<Uuid>,
    revoked: AtomicBool,
    account_disabled: AtomicBool,
    checks: AtomicUsize,
    privacy_epoch: AtomicI64,
    relationship_epoch: AtomicI64,
    source_admissions: AtomicUsize,
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
    ) -> anyhow::Result<crate::modules::memory::source::SourceEpochs> {
        let epochs = crate::modules::memory::source::SourceEpochs {
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
        Ok(serde_json::json!({"authenticated": true}))
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

async fn fixture(owner: Option<Uuid>) -> (VoiceIngressAuth, Arc<Device>, Arc<ChannelRegistry>) {
    let device = Arc::new(Device {
        owner,
        revoked: false.into(),
        account_disabled: false.into(),
        checks: 0.into(),
        privacy_epoch: 3.into(),
        relationship_epoch: 7.into(),
        source_admissions: 0.into(),
    });
    let channels = Arc::new(ChannelRegistry::new());
    channels.register(device.clone()).await.unwrap();
    (VoiceIngressAuth::new(channels.clone()), device, channels)
}

#[tokio::test]
async fn signed_device_requires_an_active_configured_owner() {
    let (auth, _, _) = fixture(None).await;
    auth.authenticate_ingress(&request(()), None).await.unwrap();
    assert!(
        matches!(auth.admit_source(DEVICE).await, Err(e) if e.code() == tonic::Code::FailedPrecondition)
    );
    let (auth, device, _) = fixture(Some(OWNER)).await;
    device.account_disabled.store(true, Ordering::SeqCst);
    assert!(
        matches!(auth.admit_source(DEVICE).await, Err(e) if e.code() == tonic::Code::PermissionDenied)
    );
}

#[tokio::test]
async fn invalid_or_revoked_credentials_never_authenticate() {
    let (auth, device, _) = fixture(Some(OWNER)).await;
    assert!(
        auth.authenticate_ingress(&Request::new(()), None)
            .await
            .is_err()
    );
    let mut invalid = request(());
    invalid
        .metadata_mut()
        .insert("authorization", "Bearer forged".parse().unwrap());
    assert!(auth.authenticate_ingress(&invalid, None).await.is_err());
    assert_eq!(
        auth.authenticate_ingress(&request(()), None).await.unwrap(),
        DEVICE
    );
    device.revoked.store(true, Ordering::SeqCst);
    assert!(auth.authenticate_ingress(&request(()), None).await.is_err());
}

#[tokio::test]
async fn source_keeps_original_epochs_and_rejects_each_revocation() {
    for privacy in [true, false] {
        let (auth, device, _) = fixture(Some(OWNER)).await;
        let source = auth.admit_source(DEVICE).await.unwrap();
        assert_eq!(
            (
                source.user_id,
                source.privacy_epoch,
                source.relationship_epoch
            ),
            (OWNER, 3, 7)
        );
        auth.revalidate_source(DEVICE, &source).await.unwrap();
        if privacy {
            device.privacy_epoch.fetch_add(1, Ordering::SeqCst);
        } else {
            device.relationship_epoch.fetch_add(1, Ordering::SeqCst);
        }
        assert!(auth.revalidate_source(DEVICE, &source).await.is_err());
        assert_eq!((source.privacy_epoch, source.relationship_epoch), (3, 7));
    }
}

#[tokio::test]
async fn removed_or_replaced_channel_cannot_lend_authority_to_a_source() {
    let (auth, _, channels) = fixture(Some(OWNER)).await;
    let source = auth.admit_source(DEVICE).await.unwrap();
    channels.unregister(DEVICE).await.unwrap();
    assert!(auth.revalidate_source(DEVICE, &source).await.is_err());
    let (_, replacement, _) = fixture(Some(OWNER)).await;
    channels.register(replacement).await.unwrap();
    assert!(auth.revalidate_source(DEVICE, &source).await.is_err());
}

#[tokio::test]
async fn voice_admission_requires_explicit_instance_id_without_type_alias_fallback() {
    let (auth, _, _) = fixture(Some(OWNER)).await;
    let mut missing = request(());
    missing.metadata_mut().remove("x-alia-channel-id");
    assert_eq!(
        auth.authenticate_ingress(&missing, None)
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    let mut alias = request(());
    alias
        .metadata_mut()
        .insert("x-alia-channel-id", "embedded".parse().unwrap());
    assert_eq!(
        auth.authenticate_ingress(&alias, None)
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(
        auth.authenticate_ingress(&request(()), None).await.unwrap(),
        DEVICE
    );
}
