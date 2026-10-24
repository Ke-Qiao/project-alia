use super::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::modules::platform::background::OwnedBackground;
use alia_proto::motion::motion_control_server::{MotionControl, MotionControlServer};
use alia_transport::{DEFAULT_AUTH_MAX_SKEW, EMBEDDED_CONTROL_SCOPE, verify_auth_token};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tonic::{Request, Response, Status};

struct DeviceState {
    control: MotionControlState,
    commands:
        HashMap<MotionCommandIdentity, (MotionCommand, MotionProgress, tokio::time::Instant, bool)>,
    requests: Vec<MotionOperation>,
    nonces: HashSet<Uuid>,
}

struct FakeDevice {
    state: Mutex<DeviceState>,
    key: SigningKey,
    server_key: Vec<u8>,
    reply_fault: AtomicUsize,
    lost_submit_ack: AtomicBool,
    lost_cancel: AtomicBool,
    block_native: AtomicBool,
    hold_submit_ack: AtomicBool,
    progress_before_ack: AtomicBool,
    entered: Semaphore,
    release: Semaphore,
}

impl FakeDevice {
    fn new(server_key: Vec<u8>) -> Self {
        Self {
            state: Mutex::new(DeviceState {
                control: MotionControlState {
                    device_boot_id: Uuid::new_v4(),
                    generation: 0,
                    lease: None,
                    available: true,
                },
                commands: HashMap::new(),
                requests: Vec::new(),
                nonces: HashSet::new(),
            }),
            key: SigningKey::from_bytes(&[31; 32]),
            server_key,
            reply_fault: AtomicUsize::new(0),
            lost_submit_ack: AtomicBool::new(false),
            lost_cancel: AtomicBool::new(false),
            block_native: AtomicBool::new(false),
            hold_submit_ack: AtomicBool::new(false),
            progress_before_ack: AtomicBool::new(false),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }

    fn operations(&self) -> Vec<MotionOperation> {
        self.state.lock().unwrap().requests.clone()
    }

    fn progress(&self, identity: MotionCommandIdentity, cancel: bool) -> MotionEvidence {
        let mut state = self.state.lock().unwrap();
        let Some((command, progress, started, cancelled)) = state.commands.get_mut(&identity)
        else {
            return MotionEvidence::Unavailable { identity };
        };
        *cancelled |= cancel;
        let previous = progress.clone();
        if progress.end.is_none() {
            let elapsed = started.elapsed().as_millis()
                >= command
                    .actions
                    .iter()
                    .map(|action| u128::from(action.duration_ms))
                    .sum::<u128>();
            if !self.block_native.load(Ordering::SeqCst) && (*cancelled || elapsed) {
                progress.writer_quiescent = true;
                progress.backend = Some(MotionBackend {
                    instance_id: identity.lease.device_boot_id,
                    boundary: MotionSubmissionBoundary::Simulated,
                    channels: 3,
                });
                if *cancelled {
                    progress.end = Some(MotionEnd::Cancelled {
                        reason: "precise software cancellation".into(),
                    });
                } else {
                    progress.started_actions = progress.accepted_actions;
                    progress.submitted_actions = progress.accepted_actions;
                    progress.submitted_frames = 20;
                    progress.end = Some(MotionEnd::Submitted);
                }
            } else {
                progress.started_actions = 1;
            }
        }
        if previous != *progress {
            progress.receipt_sequence += 1;
        }
        MotionEvidence::Progress {
            progress: progress.clone(),
        }
    }

    fn operation(&self, operation: &MotionOperation) -> Result<MotionEvidence, Status> {
        match operation {
            MotionOperation::Inspect => Ok(MotionEvidence::Control {
                state: self.state.lock().unwrap().control.clone(),
            }),
            MotionOperation::Open {
                device_boot_id,
                expected_generation,
                server_instance_id,
            } => {
                let mut state = self.state.lock().unwrap();
                if *device_boot_id != state.control.device_boot_id
                    || *expected_generation != state.control.generation
                {
                    return Err(Status::aborted("CAS lost"));
                }
                state.control.generation += 1;
                state.control.lease = Some(MotionControlLease {
                    device_boot_id: *device_boot_id,
                    server_instance_id: *server_instance_id,
                    generation: state.control.generation,
                });
                Ok(MotionEvidence::Control {
                    state: state.control.clone(),
                })
            }
            MotionOperation::Close { lease, .. } => {
                let mut state = self.state.lock().unwrap();
                if state.control.lease != Some(*lease) {
                    return Err(Status::failed_precondition("wrong original lease"));
                }
                state.control.generation += 1;
                state.control.lease = None;
                Ok(MotionEvidence::Control {
                    state: state.control.clone(),
                })
            }
            MotionOperation::Submit { command } => {
                command
                    .validate()
                    .map_err(|error| Status::invalid_argument(error.to_string()))?;
                let mut state = self.state.lock().unwrap();
                if state.control.lease != Some(command.identity.lease) {
                    return Err(Status::failed_precondition("wrong lease"));
                }
                let progress = MotionProgress {
                    identity: command.identity,
                    receipt_sequence: 1,
                    accepted_actions: command.actions.len() as u32,
                    started_actions: 0,
                    submitted_actions: 0,
                    submitted_frames: 0,
                    partial_frame: None,
                    last_partial_channels: Vec::new(),
                    last_uncertain_channels: Vec::new(),
                    backend: None,
                    writer_quiescent: false,
                    position_confirmed_actions: None,
                    end: None,
                };
                if state
                    .commands
                    .insert(
                        command.identity,
                        (
                            command.clone(),
                            progress.clone(),
                            tokio::time::Instant::now(),
                            false,
                        ),
                    )
                    .is_some()
                {
                    return Err(Status::already_exists("physical Submit repeated"));
                }
                Ok(MotionEvidence::Progress { progress })
            }
            MotionOperation::Observe { identity, .. } => Ok(self.progress(*identity, false)),
            MotionOperation::Cancel { command, .. } => Ok(self.progress(command.identity, true)),
        }
    }
}

struct DeviceService(Arc<FakeDevice>);

impl std::ops::Deref for DeviceService {
    type Target = FakeDevice;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[tonic::async_trait]
impl MotionControl for DeviceService {
    async fn exchange(
        &self,
        request: Request<SignedMotionPayload>,
    ) -> Result<Response<SignedMotionPayload>, Status> {
        let token = request
            .metadata()
            .get("authorization")
            .and_then(|header| header.to_str().ok())
            .and_then(|header| header.strip_prefix("Bearer "))
            .ok_or_else(|| Status::unauthenticated("missing real auth"))?;
        verify_auth_token(
            token,
            &self.server_key,
            "test-server",
            EMBEDDED_CONTROL_SCOPE,
            DEFAULT_AUTH_MAX_SKEW,
        )
        .map_err(|error| Status::unauthenticated(error.to_string()))?;
        let message = request.into_inner();
        verify_payload(
            &self.server_key,
            &motion_request_payload(&message.payload),
            &message.signature,
        )
        .map_err(|error| Status::unauthenticated(error.to_string()))?;
        let request: MotionRequest = serde_json::from_slice(&message.payload)
            .map_err(|error| Status::invalid_argument(error.to_string()))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        if request.version != MOTION_CONTRACT_VERSION
            || request.device_id != "trusted-device"
            || request.request_id.is_nil()
            || request.issued_at_unix_ms.abs_diff(now) > 30_000
        {
            return Err(Status::unauthenticated("wrong envelope"));
        }
        {
            let mut state = self.state.lock().unwrap();
            if !state.nonces.insert(request.request_id) {
                return Err(Status::unauthenticated("nonce reused"));
            }
            state.requests.push(request.operation.clone());
        }
        if matches!(request.operation, MotionOperation::Cancel { .. })
            && self.lost_cancel.swap(false, Ordering::SeqCst)
        {
            return Err(Status::unavailable(
                "first Cancel lost before native admission",
            ));
        }
        let mut evidence = self.operation(&request.operation)?;
        if matches!(request.operation, MotionOperation::Submit { .. }) {
            self.entered.add_permits(1);
            if self.hold_submit_ack.load(Ordering::SeqCst) {
                self.release.acquire().await.unwrap().forget();
            }
            if self.lost_submit_ack.swap(false, Ordering::SeqCst) {
                return Err(Status::unavailable(
                    "Submit admitted but acknowledgement lost",
                ));
            }
            if self.progress_before_ack.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(30)).await;
                if let MotionOperation::Submit { command } = &request.operation {
                    evidence = self.progress(command.identity, false);
                }
            }
        }
        let mut reply = MotionReply {
            version: MOTION_CONTRACT_VERSION,
            request_id: request.request_id,
            device_id: request.device_id,
            evidence,
        };
        match self.reply_fault.load(Ordering::SeqCst) {
            1 => reply.request_id = Uuid::new_v4(),
            2 => reply.device_id = "other-device".into(),
            3 => reply.version += 1,
            4 => {
                if let MotionEvidence::Progress { progress } = &mut reply.evidence {
                    progress.identity.sequence += 1;
                }
            }
            5 => {
                if let MotionEvidence::Progress { progress } = &mut reply.evidence {
                    progress.position_confirmed_actions = Some(1);
                }
            }
            _ => {}
        }
        let payload = serde_json::to_vec(&reply).unwrap();
        let key = if self.reply_fault.load(Ordering::SeqCst) == 6 {
            SigningKey::from_bytes(&[32; 32])
        } else {
            self.key.clone()
        };
        let signature = if self.reply_fault.load(Ordering::SeqCst) == 8 {
            sign_payload(&key, &motion_request_payload(&payload))
        } else {
            sign_payload(&key, &motion_reply_payload(&payload))
        };
        Ok(Response::new(SignedMotionPayload { payload, signature }))
    }
}

#[derive(Default)]
struct Store {
    records: Mutex<HashMap<Uuid, MotionDeliveryRecord>>,
    remove_after_lookup: AtomicBool,
    allow: AtomicBool,
    progress_facts: Mutex<Vec<MotionProgress>>,
    claims: AtomicUsize,
}

#[async_trait]
impl MotionDeliveryStore for Store {
    async fn lookup_origin(
        &self,
        origin: &MotionDeliveryOrigin,
        _: &str,
        _: Uuid,
    ) -> anyhow::Result<Option<MotionDeliveryRecord>> {
        let mut records = self.records.lock().unwrap();
        let record = records.get(&origin.response_id).cloned();
        if self.remove_after_lookup.swap(false, Ordering::SeqCst) {
            records.remove(&origin.response_id);
        }
        Ok(record)
    }
    async fn claim(
        &self,
        origin: &MotionDeliveryOrigin,
        _: &str,
        command: &MotionCommand,
    ) -> anyhow::Result<MotionDispatch> {
        self.claims.fetch_add(1, Ordering::SeqCst);
        let mut records = self.records.lock().unwrap();
        if let Some(record) = records.get(&origin.response_id) {
            return Ok(MotionDispatch::Existing(record.clone()));
        }
        let record = MotionDeliveryRecord {
            delivery_id: Uuid::new_v4(),
            response_id: origin.response_id,
            output_key: origin.output_key.clone(),
            dispatch_id: Uuid::new_v4(),
            command: command.clone(),
            progress: None,
            latest_checkpoint: None,
            status: "sending".into(),
            unknown_reason: None,
        };
        let fresh = MotionDispatch::Fresh {
            delivery_id: record.delivery_id,
            dispatch_id: record.dispatch_id,
        };
        records.insert(origin.response_id, record);
        Ok(fresh)
    }
    async fn authorize(&self, _: Uuid, _: Uuid) -> anyhow::Result<bool> {
        Ok(self.allow.load(Ordering::SeqCst))
    }
    async fn progress(
        &self,
        delivery: Uuid,
        _: Uuid,
        progress: &MotionProgress,
    ) -> anyhow::Result<()> {
        self.progress_facts.lock().unwrap().push(progress.clone());
        if let Some(record) = self
            .records
            .lock()
            .unwrap()
            .values_mut()
            .find(|record| record.delivery_id == delivery)
        {
            record.progress = Some(progress.clone());
        }
        Ok(())
    }
    async fn unknown(&self, delivery: Uuid, _: Uuid, reason: &str) -> anyhow::Result<()> {
        if let Some(record) = self
            .records
            .lock()
            .unwrap()
            .values_mut()
            .find(|record| record.delivery_id == delivery)
        {
            record.status = "unknown".into();
            record.unknown_reason = Some(reason.into());
        }
        Ok(())
    }
}

struct Harness {
    runtime: EmbeddedMotionRuntime,
    background: OwnedBackground,
    device: Arc<FakeDevice>,
    client: SignedMotionClient,
    store: Arc<Store>,
    server_stop: CancellationToken,
    server_task: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
}

impl Harness {
    async fn new() -> Self {
        Self::with_assets(vec![]).await
    }

    async fn with_assets(assets: Vec<MotionAnimationAsset>) -> Self {
        let server_key = SigningKey::from_bytes(&[30; 32]);
        let device = Arc::new(FakeDevice::new(
            server_key.verifying_key().to_bytes().to_vec(),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_stop = CancellationToken::new();
        let stop = server_stop.clone();
        let service = device.clone();
        let server_task = tokio::spawn(async move {
            let incoming = async_stream::stream! { loop { yield listener.accept().await.map(|(socket,_)| socket); } };
            tonic::transport::Server::builder()
                .add_service(MotionControlServer::new(DeviceService(service)))
                .serve_with_incoming_shutdown(incoming, stop.cancelled())
                .await
        });
        let transport = Channel::from_shared(format!("http://{address}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        let client = SignedMotionClient::new(
            MotionControlClient::new(transport),
            server_key,
            "test-server".into(),
            "trusted-device".into(),
            device.key.verifying_key().to_bytes().to_vec(),
        )
        .unwrap();
        let mut runtime = EmbeddedMotionRuntime::new("test-channel".into(), assets);
        Arc::get_mut(&mut runtime.0).unwrap().enabled = true;
        let store = Arc::new(Store::default());
        store.allow.store(true, Ordering::SeqCst);
        runtime.set_store(store.clone()).await;
        runtime.set_client(client.clone()).await;
        let background = OwnedBackground::default();
        let parent = CancellationToken::new();
        let mut startup = background.begin_start(&parent).await.unwrap();
        runtime.start(&mut startup).unwrap();
        startup.commit();
        Self {
            runtime,
            background,
            device,
            client,
            store,
            server_stop,
            server_task,
        }
    }

    async fn finish(self) {
        self.runtime.request_stop();
        tokio::time::timeout(Duration::from_secs(4), self.background.stop())
            .await
            .unwrap()
            .unwrap();
        self.server_stop.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.server_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    async fn retired(&self, params: &Value) -> ExpressionExecutionResult {
        let source = params["command_id"].as_str().unwrap();
        let entry = self
            .runtime
            .0
            .admission
            .lock()
            .unwrap()
            .sources
            .get(source)
            .unwrap()
            .clone();
        let mut receiver = entry.observation.subscribe();
        tokio::time::timeout(Duration::from_secs(8), async {
            while !receiver.borrow_and_update().owner_finished {
                receiver.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        entry.result(false)
    }
}

fn params(duration_ms: u32) -> Value {
    let plan = Uuid::new_v4();
    let response = Uuid::new_v4();
    json!({"id":plan,"intent":"test","social_function":"acknowledge","style":"gentle","intensity":0.3,
        "stance":crate::expression::types::ExpressiveStance::default(),"priority":"normal","interruptible":true,"safety_class":"low_risk",
        "speech":null,"timeline":[{"at":"speech_start","channel":"motion","action":"small_nod","intensity":0.3,"duration_ms":duration_ms,"params":{}}],
        "fallback":crate::expression::types::FallbackPolicy::default(),"command_id":format!("{response}:{plan}:speech_start"),"voice_scope":null,
        "target_user_id":Uuid::new_v4(),"response_id":response,"phase":"speech_start"})
}

fn owner_params(params: &Value) -> Value {
    json!({"command_id":params["command_id"],"target_user_id":params["target_user_id"],"voice_scope":params["voice_scope"],"plan_id":params["id"],"reason":"test precise cancellation"})
}

#[tokio::test]
async fn signed_exchange_rejects_wrong_nonce_device_version_and_pinned_key() {
    let h = Harness::new().await;
    for fault in [1, 2, 3, 6, 8] {
        h.device.reply_fault.store(fault, Ordering::SeqCst);
        assert!(
            h.client.exchange(MotionOperation::Inspect).await.is_err(),
            "fault {fault} was trusted"
        );
    }
    h.device.reply_fault.store(0, Ordering::SeqCst);
    assert!(h.client.exchange(MotionOperation::Inspect).await.is_ok());
    h.finish().await;
}

#[tokio::test]
async fn first_signed_snapshot_may_already_contain_complete_submission_evidence() {
    let h = Harness::new().await;
    h.device.progress_before_ack.store(true, Ordering::SeqCst);
    let params = params(1);
    h.runtime.submit(params.clone()).await.unwrap();
    let result = h.retired(&params).await;
    assert_eq!(result.status, "submitted");
    assert!(
        result
            .motion
            .as_ref()
            .unwrap()
            .progress
            .as_ref()
            .unwrap()
            .receipt_sequence
            > 1
    );
    assert_eq!(
        result
            .motion
            .as_ref()
            .and_then(|motion| motion.progress.as_ref())
            .and_then(|progress| progress.position_confirmed_actions),
        None
    );
    assert!(
        !h.device
            .operations()
            .iter()
            .any(|operation| matches!(operation, MotionOperation::Cancel { .. }))
    );
    h.finish().await;
}

#[tokio::test]
async fn five_second_command_keeps_owned_observation_after_initial_waiter_is_dropped() {
    let h = Harness::new().await;
    h.device.hold_submit_ack.store(true, Ordering::SeqCst);
    let params = params(5_000);
    let mut waiter = Box::pin(h.runtime.submit(params.clone()));
    tokio::select! { result = &mut waiter => panic!("admission unexpectedly completed: {result:?}"), _ = h.device.entered.acquire() => {} }
    drop(waiter);
    h.device.release.add_permits(1);
    let result = h.retired(&params).await;
    assert_eq!(result.status, "submitted");
    assert_eq!(
        result
            .motion
            .as_ref()
            .and_then(|motion| motion.progress.as_ref())
            .and_then(|progress| progress.position_confirmed_actions),
        None
    );
    assert!(result.motion.as_ref().unwrap().writer_retired());
    assert_eq!(
        result
            .motion
            .as_ref()
            .unwrap()
            .progress
            .as_ref()
            .unwrap()
            .position_confirmed_actions,
        None
    );
    assert_eq!(
        h.device
            .operations()
            .iter()
            .filter(|op| matches!(op, MotionOperation::Submit { .. }))
            .count(),
        1
    );
    assert!(
        h.device
            .operations()
            .iter()
            .filter(|op| matches!(op, MotionOperation::Observe { .. }))
            .count()
            > 2
    );
    h.finish().await;
}

#[tokio::test]
async fn lost_submit_ack_is_unknown_and_exact_cancel_never_replays() {
    let h = Harness::new().await;
    h.device.lost_submit_ack.store(true, Ordering::SeqCst);
    let params = params(5_000);
    h.runtime.submit(params.clone()).await.unwrap();
    let result = h.retired(&params).await;
    assert_eq!(result.status, "unknown");
    assert_eq!(
        result
            .motion
            .as_ref()
            .and_then(|motion| motion.progress.as_ref())
            .and_then(|progress| progress.position_confirmed_actions),
        None
    );
    let original = result.motion.as_ref().unwrap().command.identity;
    assert!(result.motion.as_ref().unwrap().writer_retired());
    let operations = h.device.operations();
    assert_eq!(
        operations
            .iter()
            .filter(|op| matches!(op, MotionOperation::Submit { .. }))
            .count(),
        1
    );
    assert_eq!(
        operations
            .iter()
            .filter(
                |op| matches!(op, MotionOperation::Cancel { command, .. } if command.identity == original)
            )
            .count(),
        1
    );
    h.runtime.submit(params).await.unwrap();
    assert_eq!(
        h.device
            .operations()
            .iter()
            .filter(|op| matches!(op, MotionOperation::Submit { .. }))
            .count(),
        1
    );
    assert!(
        h.store
            .records
            .lock()
            .unwrap()
            .values()
            .all(|record| record.unknown_reason.is_some())
    );
    h.finish().await;
}

#[tokio::test]
async fn first_cancel_loss_retries_original_command_without_repeating_submission() {
    let h = Harness::new().await;
    let params = params(5_000);
    h.runtime.submit(params.clone()).await.unwrap();
    h.device.lost_cancel.store(true, Ordering::SeqCst);
    h.runtime
        .observe(owner_params(&params), true)
        .await
        .unwrap();
    let result = h.retired(&params).await;
    assert_eq!(result.status, "unknown");
    let motion = result.motion.unwrap();
    assert!(motion.writer_retired());
    assert_eq!(motion.progress.as_ref().unwrap().submitted_frames, 0);
    let operations = h.device.operations();
    assert_eq!(
        operations
            .iter()
            .filter(|operation| matches!(operation, MotionOperation::Submit { .. }))
            .count(),
        1
    );
    let cancels: Vec<_> = operations
        .iter()
        .filter_map(|operation| {
            if let MotionOperation::Cancel { command, .. } = operation {
                Some(command)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(cancels.len(), 2);
    assert!(cancels.iter().all(|command| **command == motion.command));
    h.finish().await;
}

#[tokio::test]
async fn missing_trusted_pin_rejects_raw_motion_submission_before_ledger_or_control() {
    let h = Harness::new().await;
    h.runtime.revoke().await;
    assert!(h.runtime.submit(params(20)).await.is_err());
    assert!(h.device.operations().is_empty());
    assert_eq!(h.store.claims.load(Ordering::SeqCst), 0);
    assert!(h.runtime.0.admission.lock().unwrap().sources.is_empty());
    h.finish().await;
}

#[tokio::test]
async fn forged_identity_or_position_receipt_is_unknown_and_cannot_retire_the_original_writer() {
    for fault in [4, 5] {
        let h = Harness::new().await;
        h.device.reply_fault.store(fault, Ordering::SeqCst);
        let params = params(5_000);
        let result: ExpressionExecutionResult =
            serde_json::from_value(h.runtime.submit(params.clone()).await.unwrap()).unwrap();
        assert_eq!(result.status, "unknown");
        assert_eq!(
            result
                .motion
                .as_ref()
                .and_then(|motion| motion.progress.as_ref())
                .and_then(|progress| progress.position_confirmed_actions),
            None
        );
        assert!(!result.motion.as_ref().unwrap().writer_retired());
        h.device.reply_fault.store(0, Ordering::SeqCst);
        let retired = h.retired(&params).await;
        assert_eq!(retired.status, "unknown");
        assert!(retired.motion.as_ref().unwrap().writer_retired());
        assert!(
            h.store
                .progress_facts
                .lock()
                .unwrap()
                .iter()
                .all(|progress| progress.position_confirmed_actions.is_none())
        );
        assert_eq!(
            h.device
                .operations()
                .iter()
                .filter(|op| matches!(op, MotionOperation::Submit { .. }))
                .count(),
            1
        );
        h.finish().await;
    }
}

#[tokio::test]
async fn shutdown_waiter_drop_retains_owner_and_dependencies_until_writer_quiesces() {
    let h = Harness::new().await;
    h.device.block_native.store(true, Ordering::SeqCst);
    let params = params(5_000);
    h.runtime.submit(params.clone()).await.unwrap();
    h.runtime.request_stop();
    assert!(
        tokio::time::timeout(Duration::from_millis(80), h.background.stop())
            .await
            .is_err()
    );
    assert!(
        !h.device
            .operations()
            .iter()
            .any(|op| matches!(op, MotionOperation::Close { .. }))
    );
    assert!(h.runtime.submit(self::params(100)).await.is_err());
    h.device.block_native.store(false, Ordering::SeqCst);
    let result = h.retired(&params).await;
    assert_eq!(result.status, "cancelled");
    assert!(result.motion.as_ref().unwrap().writer_retired());
    assert_eq!(
        result
            .motion
            .as_ref()
            .and_then(|motion| motion.progress.as_ref())
            .and_then(|progress| progress.position_confirmed_actions),
        None
    );
    h.finish().await;
}

#[tokio::test]
async fn immutable_source_and_bounded_retirement_map_do_not_create_replay_permission() {
    let h = Harness::new().await;
    let first = params(20);
    h.runtime.submit(first.clone()).await.unwrap();
    h.retired(&first).await;
    let mut changed = first.clone();
    changed["timeline"][0]["intensity"] = json!(0.8);
    assert!(h.runtime.submit(changed).await.is_err());
    {
        let mut admission = h.runtime.0.admission.lock().unwrap();
        let original = admission.sources.values().next().unwrap().clone();
        for index in 1..MAX_SOURCES {
            admission
                .sources
                .insert(format!("retired-{index}"), original.clone());
        }
    }
    let next = params(20);
    h.runtime.submit(next.clone()).await.unwrap();
    h.retired(&next).await;
    assert_eq!(
        h.runtime.0.admission.lock().unwrap().sources.len(),
        MAX_SOURCES
    );
    // Force the original confirmed command out of the local cache. Recovery
    // consults the durable origin and cannot allocate a new device command.
    h.runtime
        .0
        .admission
        .lock()
        .unwrap()
        .sources
        .remove(first["command_id"].as_str().unwrap());
    h.runtime.submit(first).await.unwrap();
    assert_eq!(
        h.device
            .operations()
            .iter()
            .filter(|op| matches!(op, MotionOperation::Submit { .. }))
            .count(),
        2
    );
    h.finish().await;
}

#[tokio::test]
async fn missing_response_and_other_owner_cancel_have_no_physical_dispatch_authority() {
    let h = Harness::new().await;
    let mut missing = params(20);
    missing.as_object_mut().unwrap().remove("response_id");
    assert!(h.runtime.submit(missing).await.is_err());
    assert!(h.device.operations().is_empty());
    let original = params(5_000);
    h.runtime.submit(original.clone()).await.unwrap();
    let mut wrong_owner = owner_params(&original);
    wrong_owner["target_user_id"] = json!(Uuid::new_v4());
    assert!(h.runtime.observe(wrong_owner, true).await.is_err());
    assert!(
        !h.device
            .operations()
            .iter()
            .any(|op| matches!(op, MotionOperation::Cancel { .. }))
    );
    h.runtime
        .observe(owner_params(&original), true)
        .await
        .unwrap();
    h.retired(&original).await;
    h.finish().await;
}

#[tokio::test]
async fn all_users_and_voice_nonvoice_share_one_ordered_device_sequence() {
    let h = Harness::new().await;
    let a = params(20);
    let mut b = params(20);
    b["voice_scope"] = json!(VoiceScope {
        device_boot_id: h.device.state.lock().unwrap().control.device_boot_id,
        session_id: Uuid::new_v4(),
        generation_epoch: 1
    });
    let (first, second) = tokio::join!(h.runtime.submit(a.clone()), h.runtime.submit(b.clone()));
    first.unwrap();
    second.unwrap();
    h.retired(&a).await;
    h.retired(&b).await;
    let commands: Vec<_> = h
        .device
        .operations()
        .into_iter()
        .filter_map(|op| {
            if let MotionOperation::Submit { command } = op {
                Some(command)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        commands
            .iter()
            .map(|command| command.identity.sequence)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(commands[0].identity.lease, commands[1].identity.lease);
    assert_ne!(commands[0].owner_id, commands[1].owner_id);
    h.finish().await;
}

#[tokio::test]
async fn expired_idle_control_is_reopened_only_for_new_intent_and_generation_never_rewinds() {
    let h = Harness::new().await;
    let old = params(20);
    h.runtime.submit(old.clone()).await.unwrap();
    h.retired(&old).await;
    {
        let mut state = h.device.state.lock().unwrap();
        state.control.lease = None;
        state.control.generation += 1;
    }
    // Reading an old source does not reactivate its expired control lease.
    h.runtime.submit(old).await.unwrap();
    let fresh = params(20);
    h.runtime.submit(fresh.clone()).await.unwrap();
    h.retired(&fresh).await;
    let leases: Vec<_> = h
        .device
        .operations()
        .iter()
        .filter_map(|op| {
            if let MotionOperation::Submit { command } = op {
                Some(command.identity.lease.generation)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(leases, [1, 3]);
    assert_eq!(
        h.device
            .operations()
            .iter()
            .filter(|op| matches!(op, MotionOperation::Open { .. }))
            .count(),
        2
    );
    h.finish().await;
}

#[tokio::test]
async fn original_response_record_is_restored_without_opening_control_or_resubmitting() {
    for has_late in [false, true] {
        let h = Harness::new().await;
        let params = params(20);
        let intent = MotionIntent::parse(params.clone(), &[]).unwrap();
        let command = intent.command(MotionCommandIdentity {
            lease: MotionControlLease {
                device_boot_id: Uuid::new_v4(),
                server_instance_id: Uuid::new_v4(),
                generation: 2,
            },
            sequence: 40,
        });
        h.store.records.lock().unwrap().insert(
            intent.origin.response_id,
            MotionDeliveryRecord {
                delivery_id: Uuid::new_v4(),
                response_id: intent.origin.response_id,
                output_key: intent.origin.output_key,
                dispatch_id: Uuid::new_v4(),
                command: command.clone(),
                progress: None,
                latest_checkpoint: has_late.then(|| MotionProgress {
                    identity: command.identity,
                    receipt_sequence: 2,
                    accepted_actions: 1,
                    started_actions: 0,
                    submitted_actions: 0,
                    submitted_frames: 0,
                    partial_frame: None,
                    last_partial_channels: vec![],
                    last_uncertain_channels: vec![],
                    backend: None,
                    writer_quiescent: true,
                    position_confirmed_actions: None,
                    end: Some(MotionEnd::Cancelled {
                        reason: "late exact stop".into(),
                    }),
                }),
                status: "unknown".into(),
                unknown_reason: Some("former server owner expired".into()),
            },
        );
        let restored: ExpressionExecutionResult =
            serde_json::from_value(h.runtime.submit(params.clone()).await.unwrap()).unwrap();
        assert_eq!(restored.status, "unknown");
        let motion = restored.motion.unwrap();
        assert_eq!(motion.command, command);
        assert_eq!(motion.writer_retired(), has_late);
        h.retired(&params).await;
        let source = params["command_id"].as_str().unwrap();
        let entry = h
            .runtime
            .0
            .admission
            .lock()
            .unwrap()
            .sources
            .remove(source)
            .unwrap();
        assert_eq!(entry.can_retire_from_cache(), has_late);
        let mut query = owner_params(&params);
        query["motion_intent"] = params.clone();
        let observed: ExpressionExecutionResult =
            serde_json::from_value(h.runtime.observe(query, false).await.unwrap()).unwrap();
        assert_eq!(observed.status, "unknown");
        assert_eq!(observed.motion.unwrap().writer_retired(), has_late);
        assert!(h.device.operations().is_empty());
        assert_eq!(h.store.claims.load(Ordering::SeqCst), 0);
        h.finish().await;
    }
}

#[tokio::test]
async fn revoked_response_cancels_original_command_and_still_records_real_receipts() {
    let h = Harness::new().await;
    let params = params(5_000);
    h.runtime.submit(params.clone()).await.unwrap();
    h.store.allow.store(false, Ordering::SeqCst);
    let result = h.retired(&params).await;
    assert_eq!(result.status, "cancelled");
    let identity = result.motion.as_ref().unwrap().command.identity;
    assert!(
        h.store
            .progress_facts
            .lock()
            .unwrap()
            .iter()
            .any(|progress| progress.identity == identity
                && matches!(progress.end, Some(MotionEnd::Cancelled { .. })))
    );
    h.finish().await;
}

#[test]
fn animation_without_an_operator_approved_catalog_is_rejected() {
    let mut value = params(500);
    value["timeline"][0]["action"] = json!("play_animation");
    value["timeline"][0]["params"] = json!({"file":"reviewed.anim","fps":30,"loops":1});
    assert!(
        MotionIntent::parse(value, &[]).is_err(),
        "an animation absent from any operator-approved catalog was accepted"
    );
}

fn approved_asset(version: u32) -> MotionAnimationAsset {
    let content = if version == 1 {
        b"frames\n0 0 0\n"
    } else {
        b"frames\n1 0 0\n"
    };
    MotionAnimationAsset {
        file: "reviewed.anim".into(),
        version,
        sha256: motion_animation_sha256(content),
        bytes: content.len() as u32,
    }
}

fn animation_params() -> Value {
    let mut value = params(500);
    value["timeline"][0]["action"] = json!("play_animation");
    value["timeline"][0]["params"] = json!({"file":"reviewed.anim","fps":30,"loops":1});
    value
}

#[test]
fn animation_pin_is_resolved_only_from_the_approved_catalog() {
    let asset = approved_asset(1);
    let value = animation_params();
    let intent = MotionIntent::parse(value.clone(), std::slice::from_ref(&asset)).unwrap();
    assert!(
        matches!(&intent.actions[0].motion, MotionKind::Animation { asset: actual, fps: 30, loops: 1 } if actual == &asset)
    );
    for (key, supplied) in [
        ("sha256", json!(asset.sha256)),
        ("version", json!(99)),
        ("bytes", json!(1)),
        ("asset", json!(asset)),
        ("approved_animations", json!([asset.clone()])),
    ] {
        let mut forged = value.clone();
        forged["timeline"][0]["params"][key] = supplied;
        assert!(
            MotionIntent::parse(forged, std::slice::from_ref(&asset)).is_err(),
            "model supplied {key} was accepted"
        );
    }
    let mut unknown = value.clone();
    unknown["timeline"][0]["params"]["file"] = json!("unknown.anim");
    assert!(MotionIntent::parse(unknown, std::slice::from_ref(&asset)).is_err());
    let mut alias = value;
    alias["timeline"][0]["params"]["file"] = json!("reviewed");
    assert_eq!(
        MotionIntent::parse(alias, &[asset]).unwrap().actions,
        intent.actions
    );
}

#[tokio::test]
async fn a_known_durable_identity_cannot_become_fresh_when_its_row_disappears() {
    let h = Harness::new().await;
    let raw = animation_params();
    let intent = MotionIntent::parse(raw.clone(), &[approved_asset(1)]).unwrap();
    let command = intent.command(MotionCommandIdentity {
        lease: MotionControlLease {
            device_boot_id: Uuid::new_v4(),
            server_instance_id: Uuid::new_v4(),
            generation: 1,
        },
        sequence: 1,
    });
    h.store.records.lock().unwrap().insert(
        intent.origin.response_id,
        MotionDeliveryRecord {
            delivery_id: Uuid::new_v4(),
            response_id: intent.origin.response_id,
            output_key: intent.origin.output_key,
            dispatch_id: Uuid::new_v4(),
            command: command.clone(),
            progress: None,
            latest_checkpoint: None,
            status: "unknown".into(),
            unknown_reason: Some("former owner retired".into()),
        },
    );
    h.store.remove_after_lookup.store(true, Ordering::SeqCst);
    h.runtime.submit(raw.clone()).await.unwrap();
    assert_eq!(h.retired(&raw).await.motion.unwrap().command, command);
    assert!(h.store.records.lock().unwrap().is_empty());
    assert_eq!(h.store.claims.load(Ordering::SeqCst), 0);
    assert!(h.device.operations().is_empty());
    h.finish().await;
}

#[tokio::test]
async fn restarted_catalog_cannot_rebind_or_hide_the_original_motion_intent() {
    let original = approved_asset(1);
    let replacement = approved_asset(2);
    let raw = animation_params();
    let h = Harness::with_assets(vec![original.clone()]).await;
    h.runtime.submit(raw.clone()).await.unwrap();
    let first = h.retired(&raw).await;
    let command = first.motion.unwrap().command;
    assert!(
        matches!(&command.actions[0].motion, MotionKind::Animation { asset, .. } if asset == &original)
    );
    let origin: Uuid = serde_json::from_value(raw["response_id"].clone()).unwrap();
    let record = h
        .store
        .records
        .lock()
        .unwrap()
        .get(&origin)
        .unwrap()
        .clone();
    h.finish().await;

    for catalog in [vec![], vec![replacement.clone()]] {
        let h = Harness::with_assets(catalog.clone()).await;
        h.store
            .records
            .lock()
            .unwrap()
            .insert(origin, record.clone());
        let restored: ExpressionExecutionResult =
            serde_json::from_value(h.runtime.submit(raw.clone()).await.unwrap()).unwrap();
        assert_eq!(restored.motion.unwrap().command, command);
        h.retired(&raw).await;
        let mut changed = raw.clone();
        changed["timeline"][0]["params"]["fps"] = json!(60);
        assert!(h.runtime.submit(changed.clone()).await.is_err());
        h.runtime
            .0
            .admission
            .lock()
            .unwrap()
            .sources
            .remove(raw["command_id"].as_str().unwrap());
        for cancel in [false, true] {
            let mut query = owner_params(&raw);
            query["motion_intent"] = raw.clone();
            let fact: ExpressionExecutionResult =
                serde_json::from_value(h.runtime.observe(query, cancel).await.unwrap()).unwrap();
            assert_eq!(fact.motion.unwrap().command, command);
        }
        assert!(h.runtime.submit(changed.clone()).await.is_err());
        let mut changed_query = owner_params(&changed);
        changed_query["motion_intent"] = changed;
        assert!(h.runtime.observe(changed_query, false).await.is_err());
        let mut other_owner = raw.clone();
        other_owner["target_user_id"] = json!(Uuid::new_v4());
        assert!(h.runtime.submit(other_owner).await.is_err());
        assert!(
            h.device.operations().is_empty(),
            "recovery opened control or resubmitted"
        );
        assert_eq!(h.store.claims.load(Ordering::SeqCst), 0);

        let fresh = animation_params();
        if catalog.is_empty() {
            assert!(h.runtime.submit(fresh).await.is_err());
            assert!(h.device.operations().is_empty());
        } else {
            h.runtime.submit(fresh.clone()).await.unwrap();
            let next = h.retired(&fresh).await.motion.unwrap().command;
            assert!(
                matches!(&next.actions[0].motion, MotionKind::Animation { asset, .. } if asset == &replacement)
            );
            let sent: Vec<_> = h
                .device
                .operations()
                .into_iter()
                .filter_map(|op| match op {
                    MotionOperation::Submit { command } => Some(command),
                    _ => None,
                })
                .collect();
            assert_eq!(sent, vec![next]);
        }
        h.finish().await;
    }
}

#[test]
fn typed_motion_mapping_rejects_implicit_targets_nonfinite_and_unsafe_animation_names() {
    let mut value = params(500);
    value["timeline"][0]["action"] = json!("look_at_user");
    assert!(MotionIntent::parse(value.clone(), &[]).is_err());
    value["timeline"][0]["params"] = json!({"rotation":[10.0,20.0,30.0]});
    assert!(matches!(
        MotionIntent::parse(value.clone(), &[]).unwrap().actions[0].motion,
        MotionKind::LookAt { .. }
    ));
    value["timeline"][0]["params"] = json!({"rotation":[999.0,0.0,0.0]});
    assert!(MotionIntent::parse(value.clone(), &[]).is_err());
    value["timeline"][0]["action"] = json!("play_animation");
    value["timeline"][0]["params"] = json!({"file":"../unsafe.anim","fps":30,"loops":1});
    assert!(MotionIntent::parse(value, &[]).is_err());
}

struct ExpressionChannel(EmbeddedMotionRuntime);

#[async_trait]
impl crate::modules::platform::lifecycle::ModuleLifecycle for ExpressionChannel {
    fn name(&self) -> &'static str {
        "motion-expression-test"
    }
}

#[async_trait]
impl crate::modules::delivery::transport::DeliveryTransport for ExpressionChannel {
    fn channel_id(&self) -> &str {
        "test-channel"
    }
}

#[async_trait::async_trait]
impl crate::channels::adapter::ChannelAdapter for ExpressionChannel {
    fn channel_type(&self) -> &'static str {
        "test"
    }
    fn capabilities(&self) -> crate::channels::adapter::ChannelCapabilities {
        crate::channels::adapter::ChannelCapabilities {
            hardware_control: true,
            ..Default::default()
        }
    }
    async fn execute_action(
        &self,
        action: crate::channels::adapter::ChannelAction,
    ) -> anyhow::Result<Value> {
        match action.name.as_str() {
            "expression_capabilities" => self.0.capabilities().await,
            "execute_expression" => self.0.submit(action.params).await,
            "observe_expression" => self.0.observe(action.params, false).await,
            "cancel_expression" => self.0.observe(action.params, true).await,
            _ => anyhow::bail!("unknown expression test action"),
        }
    }
}

#[tokio::test]
async fn orchestrator_refreshes_accepted_motion_to_submitted_without_position_completion_or_resubmit()
 {
    use crate::expression::orchestrator::{
        ExpressionExecutionPhase, ExpressionOrchestrator,
        execute_expression_plan_phase_with_response,
    };
    let h = Harness::new().await;
    let params = params(40);
    let plan = serde_json::from_value(params.clone()).unwrap();
    let target = crate::modules::delivery::transport::DeliveryTarget {
        user_id: params["target_user_id"].as_str().unwrap().into(),
        voice: None,
    };
    let response = Some(Uuid::parse_str(params["response_id"].as_str().unwrap()).unwrap());
    let channel = ExpressionChannel(h.runtime.clone());
    let orchestrator = ExpressionOrchestrator::default();
    let accepted = execute_expression_plan_phase_with_response(
        &orchestrator,
        &channel,
        &target,
        &plan,
        ExpressionExecutionPhase::SpeechStart,
        response,
    )
    .await;
    assert_eq!(accepted.status, "accepted");
    h.retired(&params).await;
    let submitted = execute_expression_plan_phase_with_response(
        &orchestrator,
        &channel,
        &target,
        &plan,
        ExpressionExecutionPhase::SpeechStart,
        response,
    )
    .await;
    assert_eq!(submitted.status, "submitted");
    assert_eq!(
        submitted
            .motion
            .as_ref()
            .and_then(|motion| motion.progress.as_ref())
            .and_then(|progress| progress.position_confirmed_actions),
        None
    );
    assert!(submitted.motion.as_ref().unwrap().writer_retired());
    assert_eq!(
        h.device
            .operations()
            .iter()
            .filter(|operation| matches!(operation, MotionOperation::Submit { .. }))
            .count(),
        1
    );
    h.finish().await;
}
