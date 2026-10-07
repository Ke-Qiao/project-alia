//! Signed body control and the channel-owned observation lifetime.
//!
//! A transport timeout never grants permission to submit the physical command
//! again. Only a quiescent command confirmed in the durable ledger can leave
//! the bounded source cache. Reappearance always queries that original ledger.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alia_core::body::*;
use alia_core::delivery::VoiceScope;
use alia_proto::body::SignedBodyPayload;
use alia_proto::body::body_control_client::BodyControlClient;
use alia_transport::{body_reply_payload, body_request_payload, sign_payload, verify_payload};
use anyhow::{Context, ensure};
use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use tokio::sync::{Mutex as AsyncMutex, RwLock, mpsc, watch};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;
use uuid::Uuid;

use super::auth::auth_request_with_signing_key;
use crate::expression::types::{BodyExecutionEvidence, ExpressionExecutionResult};
use crate::modules::body_delivery::BodyDeliveryRepository;
pub use crate::modules::body_delivery::{BodyDeliveryOrigin, BodyDeliveryRecord, BodyDispatch};
use crate::modules::platform::background::BackgroundStart;

const MAX_SOURCES: usize = 256;
const MAX_OWNERS: usize = 32;
const RPC_TIMEOUT: Duration = Duration::from_millis(1_500);
const NATIVE_BUDGET: Duration = Duration::from_secs(30);
const OBSERVATION_BUDGET: Duration = Duration::from_secs(60);
// Production admission also requires a trusted device pin, owned runtime and
// durable Response ledger. Direct ChannelAction calls use the same gates.
const BODY_DELIVERY_ENABLED: bool = true;

/// The implementation must persist intent before returning Fresh. Each method
/// has a bounded database deadline; an ACK failure is an unknown write outcome.
#[async_trait]
pub trait BodyDeliveryStore: Send + Sync {
    async fn lookup_origin(
        &self,
        origin: &BodyDeliveryOrigin,
        channel_id: &str,
        command: &BodyCommand,
    ) -> anyhow::Result<Option<BodyDeliveryRecord>>;
    async fn claim(
        &self,
        origin: &BodyDeliveryOrigin,
        channel_id: &str,
        command: &BodyCommand,
    ) -> anyhow::Result<BodyDispatch>;
    async fn authorize(&self, delivery_id: Uuid, dispatch_id: Uuid) -> anyhow::Result<bool>;
    async fn progress(
        &self,
        delivery_id: Uuid,
        dispatch_id: Uuid,
        progress: &BodyProgress,
    ) -> anyhow::Result<()>;
    async fn unknown(
        &self,
        delivery_id: Uuid,
        dispatch_id: Uuid,
        reason: &str,
    ) -> anyhow::Result<()>;
}

#[async_trait]
impl BodyDeliveryStore for BodyDeliveryRepository {
    async fn lookup_origin(
        &self,
        origin: &BodyDeliveryOrigin,
        channel_id: &str,
        command: &BodyCommand,
    ) -> anyhow::Result<Option<BodyDeliveryRecord>> {
        BodyDeliveryRepository::lookup_origin(self, origin, channel_id, command).await
    }
    async fn claim(
        &self,
        origin: &BodyDeliveryOrigin,
        channel_id: &str,
        command: &BodyCommand,
    ) -> anyhow::Result<BodyDispatch> {
        BodyDeliveryRepository::claim(self, origin, channel_id, command).await
    }
    async fn authorize(&self, delivery_id: Uuid, dispatch_id: Uuid) -> anyhow::Result<bool> {
        BodyDeliveryRepository::authorize(self, delivery_id, dispatch_id).await
    }
    async fn progress(
        &self,
        delivery_id: Uuid,
        dispatch_id: Uuid,
        progress: &BodyProgress,
    ) -> anyhow::Result<()> {
        BodyDeliveryRepository::progress(self, delivery_id, dispatch_id, progress).await
    }
    async fn unknown(
        &self,
        delivery_id: Uuid,
        dispatch_id: Uuid,
        reason: &str,
    ) -> anyhow::Result<()> {
        BodyDeliveryRepository::unknown(self, delivery_id, dispatch_id, bounded_reason(reason))
            .await
    }
}

fn bounded_reason(reason: &str) -> &str {
    let mut end = reason.len().min(2048);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    &reason[..end]
}

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(super) struct SignedBodyClient {
    client: BodyControlClient<Channel>,
    signing_key: SigningKey,
    server_id: String,
    device_id: String,
    device_public_key: Vec<u8>,
    revoked: CancellationToken,
}

impl SignedBodyClient {
    pub(super) fn new(
        client: BodyControlClient<Channel>,
        signing_key: SigningKey,
        server_id: String,
        device_id: String,
        device_public_key: Vec<u8>,
    ) -> anyhow::Result<Self> {
        ensure!(
            !device_id.is_empty() && device_id.len() <= 256,
            "invalid body device ID"
        );
        ensure!(
            device_public_key.len() == 32,
            "body requires a pinned Ed25519 device key"
        );
        Ok(Self {
            client: client
                .max_encoding_message_size(MAX_BODY_PAYLOAD_BYTES + 128)
                .max_decoding_message_size(MAX_BODY_PAYLOAD_BYTES + 128),
            signing_key,
            server_id,
            device_id,
            device_public_key,
            revoked: CancellationToken::new(),
        })
    }

    fn same_binding(&self, other: &Self) -> bool {
        self.device_id == other.device_id
            && self.device_public_key == other.device_public_key
            && self.signing_key.verifying_key() == other.signing_key.verifying_key()
            && self.server_id == other.server_id
    }

    async fn exchange(&self, operation: BodyOperation) -> anyhow::Result<BodyEvidence> {
        let request = BodyRequest {
            version: BODY_CONTRACT_VERSION,
            request_id: Uuid::new_v4(),
            device_id: self.device_id.clone(),
            issued_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_millis()
                .try_into()?,
            operation,
        };
        let payload = serde_json::to_vec(&request)?;
        ensure!(
            payload.len() <= MAX_BODY_PAYLOAD_BYTES,
            "body request exceeds payload limit"
        );
        let signature = sign_payload(&self.signing_key, &body_request_payload(&payload));
        let message = auth_request_with_signing_key(
            &self.signing_key,
            &self.server_id,
            SignedBodyPayload { payload, signature },
        )?;
        let mut client = self.client.clone();
        let response = tokio::time::timeout(RPC_TIMEOUT, client.exchange(message))
            .await
            .context("body acknowledgement deadline elapsed")??
            .into_inner();
        ensure!(
            response.payload.len() <= MAX_BODY_PAYLOAD_BYTES,
            "body reply exceeds payload limit"
        );
        verify_payload(
            &self.device_public_key,
            &body_reply_payload(&response.payload),
            &response.signature,
        )
        .context("body reply signature mismatch")?;
        let reply: BodyReply =
            serde_json::from_slice(&response.payload).context("invalid strict body reply JSON")?;
        ensure!(
            reply.version == BODY_CONTRACT_VERSION
                && reply.request_id == request.request_id
                && reply.device_id == request.device_id,
            "body reply version, nonce or device mismatch"
        );
        match (&request.operation, &reply.evidence) {
            (
                BodyOperation::Inspect | BodyOperation::Open { .. } | BodyOperation::Close { .. },
                BodyEvidence::Control { state },
            ) => {
                ensure!(
                    !state.device_boot_id.is_nil(),
                    "body control has nil boot identity"
                );
                if let Some(lease) = state.lease {
                    ensure!(
                        lease.is_valid()
                            && lease.device_boot_id == state.device_boot_id
                            && lease.generation == state.generation,
                        "body control lease is inconsistent"
                    );
                }
                if let BodyOperation::Open {
                    device_boot_id,
                    expected_generation,
                    server_instance_id,
                } = request.operation
                {
                    let generation = expected_generation
                        .checked_add(1)
                        .context("body control generation exhausted")?;
                    ensure!(
                        state.lease
                            == Some(BodyControlLease {
                                device_boot_id,
                                server_instance_id,
                                generation
                            })
                            && state.device_boot_id == device_boot_id
                            && state.generation == generation,
                        "body Open did not acknowledge the exact CAS lease"
                    );
                }
                if let BodyOperation::Close { lease, .. } = request.operation {
                    ensure!(
                        state.device_boot_id == lease.device_boot_id
                            && state.generation > lease.generation
                            && state.lease != Some(lease),
                        "body Close did not retire the original lease"
                    );
                }
            }
            (BodyOperation::Submit { command }, BodyEvidence::Progress { progress }) => {
                progress.validate(None, command)?;
            }
            (
                BodyOperation::Observe { identity, .. }
                | BodyOperation::Cancel {
                    command: BodyCommand { identity, .. },
                    ..
                },
                BodyEvidence::Progress { progress },
            ) => {
                ensure!(
                    progress.identity == *identity,
                    "body reply changed the original command identity"
                );
            }
            (
                BodyOperation::Observe { identity, .. }
                | BodyOperation::Cancel {
                    command: BodyCommand { identity, .. },
                    ..
                },
                BodyEvidence::Unavailable { identity: received },
            ) => {
                ensure!(
                    identity == received,
                    "unavailable reply changed the original command identity"
                );
            }
            _ => anyhow::bail!("body reply evidence does not match the requested operation"),
        }
        Ok(reply.evidence)
    }
}

#[derive(Clone)]
struct BodyIntent {
    raw: serde_json::Value,
    origin: BodyDeliveryOrigin,
    source_id: String,
    plan_id: Uuid,
    owner_id: Uuid,
    scope: BodyScope,
    priority: BodyPriority,
    interruptible: bool,
    actions: Vec<BodyAction>,
    names: Vec<String>,
}

impl BodyIntent {
    fn parse(raw: serde_json::Value) -> anyhow::Result<Self> {
        ensure!(
            serde_json::to_vec(&raw)?.len() <= MAX_BODY_PAYLOAD_BYTES,
            "expression body intent exceeds its bounded cache payload"
        );
        let plan: crate::expression::types::ExpressionPlan = serde_json::from_value(raw.clone())?;
        let source_id = required_string(&raw, "command_id", 256)?;
        let owner_id = Uuid::parse_str(&required_string(&raw, "target_user_id", 64)?)?;
        let response_id = Uuid::parse_str(&required_string(&raw, "response_id", 64)?)?;
        ensure!(!response_id.is_nil(), "body requires an original Response");
        let output_key = required_string(&raw, "phase", 32)?;
        ensure!(
            matches!(
                output_key.as_str(),
                "full" | "pre_speech" | "speech_start" | "post_speech"
            ),
            "invalid body response phase"
        );
        ensure!(
            source_id == format!("{response_id}:{}:{output_key}", plan.id),
            "body source must name the canonical Response, plan and phase"
        );
        let voice: Option<VoiceScope> = serde_json::from_value(
            raw.get("voice_scope")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )?;
        let scope = match voice {
            Some(scope) => BodyScope::Voice { scope },
            // The original Response is the explicit nonvoice control scope.
            // It survives restart and is never confused with a VoiceScope.
            None => BodyScope::Nonvoice {
                scope_id: response_id,
            },
        };
        let mut actions = Vec::with_capacity(plan.timeline.len());
        let mut names = Vec::with_capacity(plan.timeline.len());
        for action in &plan.timeline {
            ensure!(
                action.channel == "motion",
                "unsupported body action channel"
            );
            let motion = match action.action.as_str() {
                "small_nod" => BodyMotion::SmallNod,
                "small_bow" => BodyMotion::SmallBow,
                "look_at_user" => BodyMotion::LookAt {
                    target: body_target(&action.params)?,
                },
                "play_animation" => BodyMotion::Animation {
                    file: required_string(&action.params, "file", 128)?,
                    fps: optional_u32(&action.params, "fps", 30)?,
                    loops: optional_u32(&action.params, "loops", 1)?,
                },
                _ => anyhow::bail!("unsupported typed body action"),
            };
            actions.push(BodyAction {
                duration_ms: action.duration_ms,
                intensity: action.intensity,
                motion,
            });
            names.push(action.action.clone());
        }
        let intent = Self {
            raw,
            origin: BodyDeliveryOrigin {
                response_id,
                output_key,
            },
            source_id,
            plan_id: plan.id,
            owner_id,
            scope,
            priority: match plan.priority {
                crate::expression::types::ExpressionPriority::Low => BodyPriority::Low,
                crate::expression::types::ExpressionPriority::Normal => BodyPriority::Normal,
                crate::expression::types::ExpressionPriority::High => BodyPriority::High,
            },
            interruptible: plan.interruptible,
            actions,
            names,
        };
        let boot = intent
            .scope
            .voice()
            .map_or_else(Uuid::new_v4, |scope| scope.device_boot_id);
        intent
            .command(BodyCommandIdentity {
                lease: BodyControlLease {
                    device_boot_id: boot,
                    server_instance_id: Uuid::new_v4(),
                    generation: 1,
                },
                sequence: 1,
            })
            .validate()?;
        Ok(intent)
    }

    fn command(&self, identity: BodyCommandIdentity) -> BodyCommand {
        BodyCommand {
            identity,
            plan_id: self.plan_id,
            source_command_id: self.source_id.clone(),
            owner_id: self.owner_id,
            scope: self.scope,
            priority: self.priority,
            interruptible: self.interruptible,
            actions: self.actions.clone(),
        }
    }

    fn verify_owner(&self, params: &serde_json::Value) -> anyhow::Result<()> {
        ensure!(
            required_string(params, "command_id", 256)? == self.source_id
                && Uuid::parse_str(&required_string(params, "target_user_id", 64)?)?
                    == self.owner_id
                && Uuid::parse_str(&required_string(params, "plan_id", 64)?)? == self.plan_id,
            "body observation/cancellation does not name the original owner"
        );
        let scope: Option<VoiceScope> = serde_json::from_value(
            params
                .get("voice_scope")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )?;
        ensure!(
            scope == self.scope.voice(),
            "body observation/cancellation changed the original scope"
        );
        Ok(())
    }
}

fn required_string(
    value: &serde_json::Value,
    field: &str,
    maximum: usize,
) -> anyhow::Result<String> {
    let value = value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .context(format!("missing body {field}"))?;
    ensure!(
        !value.trim().is_empty() && value.len() <= maximum,
        "invalid body {field}"
    );
    Ok(value.to_owned())
}

fn optional_u32(value: &serde_json::Value, field: &str, default: u32) -> anyhow::Result<u32> {
    value.get(field).map_or(Ok(default), |value| {
        serde_json::from_value(value.clone()).map_err(anyhow::Error::from)
    })
}

fn body_target(params: &serde_json::Value) -> anyhow::Result<BodyTarget> {
    let target = params.get("target").unwrap_or(params);
    if let Some(rotation) = target.get("rotation") {
        return Ok(BodyTarget::Rotation {
            degrees: serde_json::from_value(rotation.clone())?,
        });
    }
    if let Some(face) = target.get("face_center") {
        let number = |name| -> anyhow::Result<f32> {
            Ok(face
                .get(name)
                .and_then(serde_json::Value::as_f64)
                .context("missing face coordinate")? as f32)
        };
        return Ok(BodyTarget::FaceCenter {
            cx: number("cx")?,
            cy: number("cy")?,
            width: number("frame_w")?,
            height: number("frame_h")?,
        });
    }
    anyhow::bail!("look_at_user requires an explicit rotation or face target")
}

#[derive(Clone, Default)]
struct Snapshot {
    evidence: Option<BodyExecutionEvidence>,
    reason: Option<String>,
    acknowledged: bool,
    owner_finished: bool,
    durable: bool,
}

struct Entry {
    intent: BodyIntent,
    observation: watch::Sender<Snapshot>,
    cancel: CancellationToken,
    cancel_reason: Mutex<String>,
}

impl Entry {
    fn can_retire_from_cache(&self) -> bool {
        let state = self.observation.borrow();
        state.owner_finished
            && state.durable
            && state
                .evidence
                .as_ref()
                .is_some_and(BodyExecutionEvidence::writer_retired)
    }
    fn result(&self, cancellation: bool) -> ExpressionExecutionResult {
        let state = self.observation.borrow().clone();
        let names = if cancellation {
            vec!["cancel_expression".to_owned()]
        } else {
            self.intent.names.clone()
        };
        if let Some(mut evidence) = state.evidence {
            if state.reason.is_some() {
                evidence.unknown_reason = state.reason;
            }
            return ExpressionExecutionResult::from_body(evidence, &names, cancellation);
        }
        use crate::expression::types::{ExpressionActionReceipt, ExpressionActionStatus};
        ExpressionExecutionResult {
            status: "unknown".into(),
            command_id: Some(self.intent.source_id.clone()),
            voice_scope: self.intent.scope.voice(),
            body: None,
            action_receipts: names
                .into_iter()
                .map(|action| ExpressionActionReceipt {
                    action,
                    status: ExpressionActionStatus::Unknown,
                    reason: Some(
                        state
                            .reason
                            .clone()
                            .unwrap_or_else(|| "body admission has no acknowledgement".into()),
                    ),
                })
                .collect(),
            skipped_actions: Vec::new(),
            safety_modifications: Vec::new(),
        }
    }

    fn unknown(&self, reason: impl Into<String>) {
        self.observation.send_modify(|state| {
            state.reason = Some(reason.into());
            state.acknowledged = true;
        });
    }

    fn cancel(&self, reason: String) {
        *self
            .cancel_reason
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = reason;
        self.cancel.cancel();
    }
}

#[derive(Default)]
struct Cursor {
    binding: Option<(String, Vec<u8>)>,
    lease: Option<BodyControlLease>,
    uncertain_open: Option<BodyControlLease>,
    sequence: u64,
}

struct Admission {
    started: bool,
    sealed: bool,
    sources: HashMap<String, Arc<Entry>>,
}

struct Inner {
    channel_id: String,
    instance_id: Uuid,
    admission: Mutex<Admission>,
    requests: mpsc::Sender<Arc<Entry>>,
    receiver: Mutex<Option<mpsc::Receiver<Arc<Entry>>>>,
    stop: CancellationToken,
    transport: RwLock<Option<SignedBodyClient>>,
    store: RwLock<Option<Arc<dyn BodyDeliveryStore>>>,
    cursor: AsyncMutex<Cursor>,
    dispatch_gate: AsyncMutex<()>,
    enabled: bool,
}

#[derive(Clone)]
pub(super) struct EmbeddedBodyRuntime(Arc<Inner>);

impl EmbeddedBodyRuntime {
    pub(super) fn new(channel_id: String) -> Self {
        let (requests, receiver) = mpsc::channel(MAX_OWNERS);
        Self(Arc::new(Inner {
            channel_id,
            instance_id: Uuid::new_v4(),
            admission: Mutex::new(Admission {
                started: false,
                sealed: false,
                sources: HashMap::new(),
            }),
            requests,
            receiver: Mutex::new(Some(receiver)),
            stop: CancellationToken::new(),
            transport: RwLock::new(None),
            store: RwLock::new(None),
            cursor: AsyncMutex::new(Cursor::default()),
            dispatch_gate: AsyncMutex::new(()),
            enabled: BODY_DELIVERY_ENABLED,
        }))
    }

    pub(super) async fn set_store(&self, store: Arc<dyn BodyDeliveryStore>) {
        *self.0.store.write().await = Some(store);
    }

    pub(super) async fn set_client(&self, mut client: SignedBodyClient) {
        let mut transport = self.0.transport.write().await;
        if let Some(previous) = transport.as_ref() {
            if previous.same_binding(&client) {
                client.revoked = previous.revoked.clone();
            } else {
                previous.revoked.cancel();
            }
        }
        *transport = Some(client);
    }

    pub(super) async fn revoke(&self) {
        if let Some(client) = self.0.transport.write().await.take() {
            client.revoked.cancel();
        }
    }

    #[cfg(test)]
    pub(super) async fn has_pinned_transport(&self) -> bool {
        self.0.transport.read().await.is_some()
    }

    pub(super) fn start(&self, background: &mut BackgroundStart<'_>) -> anyhow::Result<()> {
        let mut admission = self
            .0
            .admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        ensure!(
            !admission.started && !admission.sealed,
            "body runtime already started or sealed"
        );
        let receiver = self
            .0
            .receiver
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .context("body owner receiver absent")?;
        admission.started = true;
        let owner = self.clone();
        let parent = background.token();
        background.spawn_owned("embedded body observation owner", async move {
            owner.run(receiver, parent).await
        });
        Ok(())
    }

    pub(super) fn request_stop(&self) {
        self.0
            .admission
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .sealed = true;
        self.0.stop.cancel();
    }

    /// This is a capability gate, not a native safety or position measurement.
    pub(super) async fn capabilities(&self) -> anyhow::Result<serde_json::Value> {
        let ready = self.0.enabled && self.0.store.read().await.is_some() && {
            let admission = self
                .0
                .admission
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            admission.started
                && !admission.sealed
                && (admission.sources.len() < MAX_SOURCES
                    || admission
                        .sources
                        .values()
                        .any(|entry| entry.can_retire_from_cache()))
        };
        let client = self.0.transport.read().await.clone();
        let state = if let Some(client) = client.filter(|_| ready) {
            match client.exchange(BodyOperation::Inspect).await? {
                BodyEvidence::Control { state } => Some(state),
                _ => unreachable!(),
            }
        } else {
            None
        };
        let available = state.as_ref().is_some_and(|state| state.available);
        let protocol =
            available.then_some(crate::expression::orchestrator::EXPRESSION_COMMAND_PROTOCOL);
        Ok(
            serde_json::json!({"adapter":"embedded_body_v1", "command_protocol":protocol,
            "capabilities": {
                "small_nod":{"supported":available,"max_intensity":0.45,"max_duration_ms":900},
                "small_bow":{"supported":available,"max_intensity":0.35,"max_duration_ms":1200},
                "look_at_user":{"supported":available,"max_intensity":0.45,"max_duration_ms":1200},
                "play_animation":{"supported":available,"max_intensity":0.45,"max_duration_ms":5000}},
            "current_state":{"busy":state.is_some() && !available,"motion_adapter_registered":available},
            "position_feedback":null}),
        )
    }

    pub(super) async fn submit(
        &self,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        ensure!(self.0.enabled, "body delivery integration is not enabled");
        ensure!(
            self.0
                .transport
                .read()
                .await
                .as_ref()
                .is_some_and(|client| !client.revoked.is_cancelled()),
            "body control requires a configured trusted device ID and public key that match the connected device"
        );
        ensure!(
            self.0.store.read().await.is_some(),
            "body durable delivery store is unavailable"
        );
        let intent = BodyIntent::parse(params)?;
        let entry = {
            let mut admission = self
                .0
                .admission
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            ensure!(
                admission.started && !admission.sealed,
                "body admission is closed"
            );
            if let Some(existing) = admission.sources.get(&intent.source_id) {
                ensure!(
                    existing.intent.raw == intent.raw,
                    "body source identity was reused with different intent"
                );
                existing.clone()
            } else {
                if admission.sources.len() >= MAX_SOURCES {
                    let retired = admission.sources.iter().find_map(|(key, entry)| entry.can_retire_from_cache().then_some(key.clone()))
                        .context("body owner capacity exhausted; unconfirmed identities cannot be forgotten")?;
                    admission.sources.remove(&retired);
                }
                let (observation, _) = watch::channel(Snapshot::default());
                let entry = Arc::new(Entry {
                    intent,
                    observation,
                    cancel: CancellationToken::new(),
                    cancel_reason: Mutex::new("body command cancelled".into()),
                });
                self.0
                    .requests
                    .try_send(entry.clone())
                    .context("body owner queue is full or closed")?;
                admission
                    .sources
                    .insert(entry.intent.source_id.clone(), entry.clone());
                entry
            }
        };
        let mut observation = entry.observation.subscribe();
        let _ = tokio::time::timeout(RPC_TIMEOUT, async {
            while !observation.borrow_and_update().acknowledged {
                observation.changed().await?;
            }
            Ok::<(), watch::error::RecvError>(())
        })
        .await;
        Ok(serde_json::to_value(entry.result(false))?)
    }

    pub(super) async fn observe(
        &self,
        params: serde_json::Value,
        cancel: bool,
    ) -> anyhow::Result<serde_json::Value> {
        let source = required_string(&params, "command_id", 256)?;
        let entry = self
            .0
            .admission
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .sources
            .get(&source)
            .cloned();
        let Some(entry) = entry else {
            let intent = BodyIntent::parse(
                params
                    .get("body_intent")
                    .cloned()
                    .context("original body intent is unavailable; resubmission is forbidden")?,
            )?;
            intent.verify_owner(&params)?;
            let store = self
                .0
                .store
                .read()
                .await
                .clone()
                .context("body delivery ledger unavailable")?;
            let placeholder = intent.command(BodyCommandIdentity {
                lease: BodyControlLease {
                    device_boot_id: intent
                        .scope
                        .voice()
                        .map_or_else(Uuid::new_v4, |scope| scope.device_boot_id),
                    server_instance_id: self.0.instance_id,
                    generation: 1,
                },
                sequence: 1,
            });
            let record = store
                .lookup_origin(&intent.origin, &self.0.channel_id, &placeholder)
                .await?
                .context(
                    "original durable body record is unavailable; resubmission is forbidden",
                )?;
            let evidence = BodyExecutionEvidence {
                command: record.command,
                progress: record.latest_checkpoint.or(record.progress),
                unknown_reason: record.unknown_reason,
            };
            evidence.validate()?;
            let names = if cancel {
                vec!["cancel_expression".into()]
            } else {
                intent.names
            };
            return Ok(serde_json::to_value(ExpressionExecutionResult::from_body(
                evidence, &names, cancel,
            ))?);
        };
        entry.intent.verify_owner(&params)?;
        if cancel {
            let reason = required_string(&params, "reason", 1024)?;
            entry.cancel(reason);
            let mut observation = entry.observation.subscribe();
            let _ = tokio::time::timeout(RPC_TIMEOUT, async {
                loop {
                    let state = observation.borrow_and_update().clone();
                    if state.owner_finished
                        || state
                            .evidence
                            .as_ref()
                            .is_some_and(BodyExecutionEvidence::writer_retired)
                    {
                        break;
                    }
                    observation.changed().await?;
                }
                Ok::<(), watch::error::RecvError>(())
            })
            .await;
        }
        Ok(serde_json::to_value(entry.result(cancel))?)
    }

    async fn identity(&self, client: &SignedBodyClient) -> anyhow::Result<BodyCommandIdentity> {
        let mut cursor = self.0.cursor.lock().await;
        let binding = (client.device_id.clone(), client.device_public_key.clone());
        if cursor.binding.as_ref() != Some(&binding) {
            *cursor = Cursor {
                binding: Some(binding),
                ..Default::default()
            };
        }
        let BodyEvidence::Control { state } = client.exchange(BodyOperation::Inspect).await? else {
            unreachable!()
        };
        if let Some(lease) = cursor.lease
            && state.lease != Some(lease)
        {
            ensure!(
                state.device_boot_id != lease.device_boot_id
                    || state.generation >= lease.generation,
                "signed body control generation regressed within one boot"
            );
            ensure!(
                state.available,
                "original body lease changed while writer is unavailable"
            );
            cursor.lease = None;
            cursor.sequence = 0;
        }
        if cursor.lease.is_none() {
            if let Some(uncertain) = cursor.uncertain_open {
                ensure!(
                    state.lease == Some(uncertain),
                    "prior body Open has no recoverable exact lease; opening another would hide uncertainty"
                );
                cursor.lease = Some(uncertain);
                cursor.uncertain_open = None;
            } else {
                ensure!(state.available, "device body writer is unavailable");
                let lease = BodyControlLease {
                    device_boot_id: state.device_boot_id,
                    server_instance_id: self.0.instance_id,
                    generation: state
                        .generation
                        .checked_add(1)
                        .context("body generation exhausted")?,
                };
                cursor.uncertain_open = Some(lease);
                client
                    .exchange(BodyOperation::Open {
                        device_boot_id: state.device_boot_id,
                        expected_generation: state.generation,
                        server_instance_id: self.0.instance_id,
                    })
                    .await?;
                cursor.lease = Some(lease);
                cursor.uncertain_open = None;
            }
        }
        cursor.sequence = cursor
            .sequence
            .checked_add(1)
            .context("body sequence exhausted")?;
        Ok(BodyCommandIdentity {
            lease: cursor.lease.unwrap(),
            sequence: cursor.sequence,
        })
    }

    async fn run(
        &self,
        mut receiver: mpsc::Receiver<Arc<Entry>>,
        parent: CancellationToken,
    ) -> anyhow::Result<()> {
        let mut jobs = JoinSet::new();
        let mut stopping = false;
        let mut failures = Vec::new();
        loop {
            if stopping && jobs.is_empty() && receiver.is_empty() {
                break;
            }
            tokio::select! {
                biased;
                _ = parent.cancelled(), if !stopping => { self.request_stop(); stopping = true; receiver.close(); },
                _ = self.0.stop.cancelled(), if !stopping => { stopping = true; receiver.close(); },
                result = jobs.join_next(), if !jobs.is_empty() => {
                    if let Some(Err(error)) = result {
                        failures.push(format!("body owner panicked: {error}"));
                        self.request_stop();
                        // No proof remains that the panic retired the original
                        // device writer. Keep the channel dependency owned.
                        futures::future::pending::<()>().await;
                    }
                },
                entry = receiver.recv(), if jobs.len() < MAX_OWNERS && !(stopping && receiver.is_empty()) => {
                    if let Some(entry) = entry {
                        if stopping { entry.unknown("body runtime stopped before dispatch"); entry.observation.send_modify(|state| state.owner_finished = true); }
                        else {
                            let runtime = self.clone();
                            jobs.spawn(async move { runtime.own_command(entry).await });
                        }
                    } else if !stopping { stopping = true; }
                },
            }
        }
        if let (Some(client), Some(lease)) = (
            self.0.transport.read().await.clone(),
            self.0.cursor.lock().await.lease,
        ) {
            if let Err(error) = client
                .exchange(BodyOperation::Close {
                    lease,
                    reason: "server body owner shutdown".into(),
                })
                .await
            {
                failures.push(format!("close original body lease: {error:#}"));
            }
        }
        ensure!(
            failures.is_empty(),
            "body owner shutdown failed: {}",
            failures.join("; ")
        );
        Ok(())
    }

    async fn own_command(&self, entry: Arc<Entry>) {
        if let Err(error) = self.dispatch_and_observe(&entry).await {
            entry.unknown(format!(
                "body command owner could not establish an observation: {error:#}"
            ));
        }
        entry
            .observation
            .send_modify(|state| state.owner_finished = true);
    }

    async fn dispatch_and_observe(&self, entry: &Entry) -> anyhow::Result<()> {
        let store = self
            .0
            .store
            .read()
            .await
            .clone()
            .context("body delivery ledger unavailable")?;
        let placeholder = entry.intent.command(BodyCommandIdentity {
            lease: BodyControlLease {
                device_boot_id: entry
                    .intent
                    .scope
                    .voice()
                    .map_or_else(Uuid::new_v4, |scope| scope.device_boot_id),
                server_instance_id: self.0.instance_id,
                generation: 1,
            },
            sequence: 1,
        });
        if let Some(record) = store
            .lookup_origin(&entry.intent.origin, &self.0.channel_id, &placeholder)
            .await?
        {
            return Self::restore_existing(entry, record);
        }
        // Sequence allocation, persistence and the first transport admission
        // stay ordered across all users and voice/nonvoice producers.
        let dispatch = self.0.dispatch_gate.lock().await;
        let client = self
            .0
            .transport
            .read()
            .await
            .clone()
            .context("signed body transport unavailable")?;
        ensure!(
            !client.revoked.is_cancelled()
                && !self.0.stop.is_cancelled()
                && !entry.cancel.is_cancelled(),
            "body admission was revoked before dispatch"
        );
        let command = entry.intent.command(self.identity(&client).await?);
        command.validate()?;
        entry.observation.send_modify(|state| {
            state.evidence = Some(BodyExecutionEvidence {
                command: command.clone(),
                progress: None,
                unknown_reason: None,
            })
        });
        let (delivery_id, dispatch_id) = match store
            .claim(&entry.intent.origin, &self.0.channel_id, &command)
            .await?
        {
            BodyDispatch::Fresh {
                delivery_id,
                dispatch_id,
            } => {
                entry.observation.send_modify(|state| state.durable = true);
                (delivery_id, dispatch_id)
            }
            BodyDispatch::Existing(record) => return Self::restore_existing(entry, record),
        };
        if !store.authorize(delivery_id, dispatch_id).await?
            || self.0.stop.is_cancelled()
            || entry.cancel.is_cancelled()
            || client.revoked.is_cancelled()
        {
            store
                .unknown(
                    delivery_id,
                    dispatch_id,
                    "body dispatch revoked before transport submission",
                )
                .await?;
            anyhow::bail!("body dispatch revoked before transport submission");
        }
        // From this await onwards the device may own the command. Every path
        // continues precise observation/cancellation; it never retries Submit.
        let mut evidence = client
            .exchange(BodyOperation::Submit {
                command: command.clone(),
            })
            .await;
        drop(dispatch);
        let mut previous: Option<BodyProgress> = None;
        let started = tokio::time::Instant::now();
        let mut cancelled = false;
        let mut cancellation_reason = String::new();
        let mut uncertainty_recorded = false;
        loop {
            let mut fault = None;
            match evidence {
                Ok(BodyEvidence::Progress { progress }) => {
                    let duplicate = previous.as_ref() == Some(&progress);
                    let valid = if duplicate {
                        Ok(())
                    } else {
                        progress
                            .validate(previous.as_ref(), &command)
                            .map_err(anyhow::Error::from)
                    };
                    match valid {
                        Err(error) => {
                            fault = Some(format!("invalid original body progress: {error:#}"))
                        }
                        Ok(()) => {
                            // Keep the latest verified device fact even when its
                            // database ACK is lost. The next receipt must still
                            // be checked against this fact, not an older row.
                            previous = Some(progress.clone());
                            match store.progress(delivery_id, dispatch_id, &progress).await {
                                Ok(()) => {
                                    let retired =
                                        progress.writer_quiescent && progress.end.is_some();
                                    entry.observation.send_modify(|state| {
                                        state.evidence.as_mut().unwrap().progress = Some(progress);
                                        state.acknowledged = true;
                                    });
                                    if retired {
                                        return Ok(());
                                    }
                                }
                                Err(error) => {
                                    fault = Some(format!(
                                        "body progress persistence outcome unknown: {error:#}"
                                    ))
                                }
                            }
                        }
                    }
                }
                Ok(BodyEvidence::Unavailable { .. }) => {
                    fault = Some("original body evidence was retired; replay is forbidden".into())
                }
                Ok(_) => {
                    fault =
                        Some("unexpected control evidence while observing original command".into())
                }
                Err(error) => fault = Some(format!("body observation failed: {error:#}")),
            }
            let revoked = match store.authorize(delivery_id, dispatch_id).await {
                Ok(allowed) => !allowed,
                Err(error) => {
                    fault.get_or_insert_with(|| {
                        format!("body authorization check failed: {error:#}")
                    });
                    true
                }
            };
            let stopping = self.0.stop.is_cancelled()
                || client.revoked.is_cancelled()
                || entry.cancel.is_cancelled();
            let expired = started.elapsed() >= NATIVE_BUDGET;
            if let Some(reason) = fault {
                entry.unknown(reason.clone());
                if !uncertainty_recorded {
                    uncertainty_recorded = store
                        .unknown(delivery_id, dispatch_id, &reason)
                        .await
                        .is_ok();
                }
            }
            if !cancelled
                && (stopping || revoked || expired || entry.observation.borrow().reason.is_some())
            {
                cancelled = true;
                let reason = if stopping {
                    entry
                        .cancel_reason
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .clone()
                } else if revoked {
                    "original body response admission revoked".into()
                } else if expired {
                    "body native budget exhausted".into()
                } else {
                    "body observation outcome unknown".into()
                };
                cancellation_reason = reason.clone();
                evidence = client
                    .exchange(BodyOperation::Cancel {
                        command: command.clone(),
                        reason,
                    })
                    .await;
                continue;
            }
            if started.elapsed() >= OBSERVATION_BUDGET {
                entry.unknown("body writer quiescence remains unknown after observation budget; owner retained");
                if !uncertainty_recorded {
                    uncertainty_recorded = store
                        .unknown(
                            delivery_id,
                            dispatch_id,
                            "body observation budget exhausted",
                        )
                        .await
                        .is_ok();
                }
            }
            let delay = if started.elapsed() >= OBSERVATION_BUDGET {
                Duration::from_secs(1)
            } else {
                Duration::from_millis(250)
            };
            if !cancelled {
                tokio::select! { _ = tokio::time::sleep(delay) => {}, _ = self.0.stop.cancelled() => {}, _ = client.revoked.cancelled() => {}, _ = entry.cancel.cancelled() => {} }
            } else {
                tokio::time::sleep(delay).await;
            }
            evidence = client
                .exchange(if cancelled {
                    // The first Cancel may have been lost. Idempotent, bounded
                    // retries carry the full original command to fence even a
                    // delayed Submit still being prepared by the remote device.
                    BodyOperation::Cancel {
                        command: command.clone(),
                        reason: cancellation_reason.clone(),
                    }
                } else {
                    BodyOperation::Observe {
                        identity: command.identity,
                        after_receipt_sequence: previous
                            .as_ref()
                            .map_or(0, |progress| progress.receipt_sequence),
                    }
                })
                .await;
        }
    }

    fn restore_existing(entry: &Entry, record: BodyDeliveryRecord) -> anyhow::Result<()> {
        record.command.validate()?;
        let progress = record.latest_checkpoint.or(record.progress);
        if let Some(progress) = &progress {
            progress.validate(None, &record.command)?;
        }
        entry.observation.send_modify(|state| {
            state.evidence = Some(BodyExecutionEvidence {
                command: record.command,
                progress,
                unknown_reason: record.unknown_reason,
            });
            state.acknowledged = true;
            state.durable = true;
        });
        Ok(())
    }
}
