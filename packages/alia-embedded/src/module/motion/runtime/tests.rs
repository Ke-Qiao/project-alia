use super::*;
use alia_core::body::{BodyAction, BodyMotion, BodyPriority, BodyTarget};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::module::motion::servo::{ServoBoundary, ServoWrite};

const CALIBRATION: &str = r#"{
  "ServoMinMax": {"servo_0": [0,180], "servo_1": [0,180], "servo_2": [0,180]},
  "NeckAngles": [[0,0,0]], "MatrixSize": [1,3], "sigma": 20.0,
  "Matrix": [[90,90,90]], "NeckAngleMinMax": [-30,30,-30,30,-30,30]
}"#;

#[derive(Default)]
struct ProbeGate {
    block: bool,
    entered: bool,
}

#[derive(Default)]
struct ProbeServo {
    gate: Mutex<ProbeGate>,
    released: Condvar,
    writes: Mutex<Vec<(Instant, Vec<f32>)>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
    partial: AtomicBool,
    panic: AtomicBool,
}

struct NativeBlockGuard(Arc<ProbeServo>);
impl Drop for NativeBlockGuard {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct PreparationRelease(Arc<(Mutex<bool>, Condvar)>);
impl Drop for PreparationRelease {
    fn drop(&mut self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
}

impl ProbeServo {
    fn block_next(self: &Arc<Self>) -> NativeBlockGuard {
        *self.gate.lock().unwrap() = ProbeGate {
            block: true,
            entered: false,
        };
        NativeBlockGuard(self.clone())
    }
    fn release(&self) {
        self.gate.lock().unwrap().block = false;
        self.released.notify_all();
    }
    fn count(&self) -> usize {
        self.writes.lock().unwrap().len()
    }
    async fn wait_entered(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !self.gate.lock().unwrap().entered {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("native call entered");
    }
}

impl ServoBackend for ProbeServo {
    fn submit_frame(&self, angles: &[f32], cancelled: &dyn Fn() -> bool) -> ServoWrite {
        if cancelled() {
            return ServoWrite {
                boundary: ServoBoundary::Simulated,
                submitted_channels: vec![],
                uncertain_channels: vec![],
                accepted_bytes: 0,
                error: None,
                cancelled: true,
            };
        }
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_active.fetch_max(active, Ordering::AcqRel);
        let mut gate = self.gate.lock().unwrap();
        if gate.block {
            gate.entered = true;
            while gate.block {
                gate = self.released.wait(gate).unwrap();
            }
        }
        drop(gate);
        self.writes
            .lock()
            .unwrap()
            .push((Instant::now(), angles.to_vec()));
        self.active.fetch_sub(1, Ordering::AcqRel);
        assert!(
            !self.panic.swap(false, Ordering::AcqRel),
            "injected native panic"
        );
        if self.partial.swap(false, Ordering::AcqRel) {
            return ServoWrite {
                boundary: ServoBoundary::I2cRegisterWrite,
                submitted_channels: vec![0],
                uncertain_channels: vec![1],
                accepted_bytes: 5,
                error: Some("injected partial I2C transaction".into()),
                cancelled: false,
            };
        }
        // An already entered syscall may finish after cancellation. Its complete
        // prefix is factual and must survive the fence, never be fabricated away.
        ServoWrite {
            boundary: ServoBoundary::Simulated,
            submitted_channels: (0..angles.len() as u32).collect(),
            uncertain_channels: vec![],
            accepted_bytes: 0,
            error: None,
            cancelled: false,
        }
    }
}

fn unstarted() -> (MotionRuntime, Arc<ProbeServo>) {
    let config = MotionConfig {
        servo_count: 3,
        fps: 120,
        calibration_confirmed: true,
        ..MotionConfig::default()
    };
    let servo = Arc::new(ProbeServo::default());
    let runtime = MotionRuntime::from_parts(
        config,
        Uuid::new_v4(),
        Some(NeckMapper::from_json(CALIBRATION).unwrap()),
        servo.clone(),
    );
    (runtime, servo)
}

fn authorization() -> BodyAuthorization {
    BodyAuthorization {
        expires_at: Instant::now() + Duration::from_secs(30),
    }
}

fn open(runtime: &MotionRuntime) -> BodyControlLease {
    runtime
        .open_control(
            runtime.control_state().generation,
            Uuid::new_v4(),
            authorization(),
        )
        .unwrap()
}

fn command(lease: BodyControlLease, sequence: u64) -> BodyCommand {
    BodyCommand {
        identity: BodyCommandIdentity { lease, sequence },
        plan_id: Uuid::new_v4(),
        source_command_id: format!("test:{sequence}"),
        owner_id: Uuid::new_v4(),
        scope: BodyScope::Nonvoice {
            scope_id: Uuid::new_v4(),
        },
        priority: BodyPriority::Normal,
        interruptible: true,
        actions: vec![BodyAction {
            duration_ms: 1,
            intensity: 0.1,
            motion: BodyMotion::LookAt {
                target: BodyTarget::Rotation {
                    degrees: [1.0, 0.0, 0.0],
                },
            },
        }],
    }
}

async fn prepared(runtime: &MotionRuntime, command: &BodyCommand) -> PreparedBodyCommand {
    runtime
        .prepare(command.clone(), serde_json::to_vec(command).unwrap())
        .await
        .unwrap()
}

async fn submitted(runtime: &MotionRuntime, command: &BodyCommand) -> BodyObservation {
    runtime
        .submit(prepared(runtime, command).await, authorization())
        .unwrap()
}

async fn terminal(mut observation: BodyObservation, command: &BodyCommand) -> BodyProgress {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut previous = None;
        loop {
            let next = observation.snapshot();
            if previous.as_ref().is_none_or(|previous: &BodyProgress| {
                previous.receipt_sequence != next.receipt_sequence
            }) {
                next.validate(previous.as_ref(), command).unwrap();
                previous = Some(next.clone());
            }
            if next.end.is_some() {
                return next;
            }
            observation.changed().await.unwrap();
        }
    })
    .await
    .expect("body command terminal evidence")
}

#[tokio::test]
async fn submitted_receipts_are_monotonic_and_do_not_claim_motor_position() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let lease = open(&runtime);
    let command = command(lease, 1);
    let observation = submitted(&runtime, &command).await;
    let initial = observation.snapshot();
    assert!(initial.receipt_sequence >= 1);
    assert_eq!(initial.accepted_actions, 1);
    if initial.receipt_sequence == 1 {
        assert!(!initial.writer_quiescent);
    }
    let end = terminal(observation, &command).await;
    assert_eq!(end.end, Some(BodyEnd::Submitted));
    assert_eq!(end.submitted_actions, 1);
    assert_eq!(end.submitted_frames, 1);
    assert!(end.position_confirmed_actions.is_none());
    assert_eq!(
        end.backend.unwrap().boundary,
        alia_core::body::BodySubmissionBoundary::Simulated
    );
    assert_eq!(servo.count(), 2);
    assert_eq!(servo.max_active.load(Ordering::Acquire), 1);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn duplicate_identity_reuses_original_receipt_and_changed_intent_is_rejected() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let command = command(open(&runtime), 1);
    let first = terminal(submitted(&runtime, &command).await, &command).await;
    let replay = submitted(&runtime, &command).await.snapshot();
    assert_eq!(first, replay);
    let mut changed = command.clone();
    changed.owner_id = Uuid::new_v4();
    assert!(
        runtime
            .submit(prepared(&runtime, &changed).await, authorization())
            .is_err()
    );
    assert_eq!(servo.count(), 2);
    let forged = serde_json::to_vec(&command).unwrap();
    assert!(runtime.prepare(changed, forged).await.is_err());
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn proven_rejection_consumes_identity_and_cannot_turn_into_late_submission() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let command = command(open(&runtime), 1);
    let late = prepared(&runtime, &command).await;
    let canonical = serde_json::to_vec(&command).unwrap();
    let rejected = runtime
        .reject(
            command.clone(),
            canonical.clone(),
            "device refused new intent",
        )
        .unwrap()
        .snapshot();
    rejected.validate(None, &command).unwrap();
    assert_eq!(rejected.receipt_sequence, 1);
    assert_eq!(rejected.accepted_actions, 0);
    assert_eq!(rejected.started_actions, 0);
    assert_eq!(rejected.submitted_frames, 0);
    assert!(rejected.writer_quiescent);
    assert!(matches!(rejected.end, Some(BodyEnd::Rejected { .. })));
    assert_eq!(
        runtime.submit(late, authorization()).unwrap().snapshot(),
        rejected
    );
    assert_eq!(
        runtime
            .reject(command, canonical, "different rejection reason")
            .unwrap()
            .snapshot(),
        rejected
    );
    assert_eq!(servo.count(), 1);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn rejection_must_return_existing_live_and_terminal_facts_without_replacing_them() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let command = command(open(&runtime), 1);
    let canonical = serde_json::to_vec(&command).unwrap();
    let _native_guard = servo.block_next();
    let original = submitted(&runtime, &command).await;
    servo.wait_entered().await;
    let returned = runtime
        .reject(
            command.clone(),
            canonical.clone(),
            "a retry failed preparation",
        )
        .unwrap();
    assert_eq!(returned.snapshot(), original.snapshot());
    assert!(returned.snapshot().end.is_none());
    let mut different = command.clone();
    different.owner_id = Uuid::new_v4();
    assert!(
        runtime
            .reject(
                different.clone(),
                serde_json::to_vec(&different).unwrap(),
                "different intent"
            )
            .is_err()
    );
    servo.release();
    let end = terminal(original, &command).await;
    assert_eq!(end.end, Some(BodyEnd::Submitted));
    assert_eq!(
        runtime
            .reject(command, canonical, "later retry failed")
            .unwrap()
            .snapshot(),
        end
    );
    assert_eq!(servo.count(), 2);
    runtime.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_submit_and_reject_publish_one_immutable_outcome() {
    let (runtime, servo) = unstarted();
    let runtime = Arc::new(runtime);
    runtime.start().await.unwrap();
    let lease = open(&runtime);
    let mut submitted_count = 0;
    for sequence in 1..=12 {
        let command = command(lease, sequence);
        let prepared = prepared(&runtime, &command).await;
        let canonical = serde_json::to_vec(&command).unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let submit = tokio::spawn({
            let runtime = runtime.clone();
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                runtime.submit(prepared, authorization())
            }
        });
        let reject = tokio::spawn({
            let runtime = runtime.clone();
            let barrier = barrier.clone();
            let command = command.clone();
            async move {
                barrier.wait().await;
                runtime.reject(command, canonical, "concurrent policy refusal")
            }
        });
        let end = terminal(submit.await.unwrap().unwrap(), &command).await;
        let other_end = terminal(reject.await.unwrap().unwrap(), &command).await;
        assert_eq!(end, other_end);
        match end.end {
            Some(BodyEnd::Submitted) => submitted_count += 1,
            Some(BodyEnd::Rejected { .. }) => assert_eq!(end.submitted_frames, 0),
            other => panic!("unexpected concurrent outcome {other:?}"),
        }
    }
    assert_eq!(servo.count(), 1 + submitted_count);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn rejection_fences_preparation_that_has_not_yet_returned_to_its_rpc_waiter() {
    let (mut runtime, servo) = unstarted();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let _preparation_release = PreparationRelease(gate.clone());
    let entered = Arc::new(AtomicBool::new(false));
    runtime.preparation_hook = Some(Arc::new({
        let gate = gate.clone();
        let entered = entered.clone();
        move || {
            entered.store(true, Ordering::Release);
            let mut released = gate.0.lock().unwrap();
            while !*released {
                released = gate.1.wait(released).unwrap();
            }
        }
    }));
    let runtime = Arc::new(runtime);
    runtime.start().await.unwrap();
    let command = command(open(&runtime), 1);
    let canonical = serde_json::to_vec(&command).unwrap();
    let preparation = tokio::spawn({
        let runtime = runtime.clone();
        let command = command.clone();
        let canonical = canonical.clone();
        async move { runtime.prepare(command, canonical).await }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !entered.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    let rejected = runtime
        .reject(
            command.clone(),
            canonical,
            "request admission expired while preparing",
        )
        .unwrap()
        .snapshot();
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let late = preparation.await.unwrap().unwrap();
    assert_eq!(
        runtime.submit(late, authorization()).unwrap().snapshot(),
        rejected
    );
    assert_eq!(servo.count(), 1);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn rejected_receipt_eviction_preserves_waterline_and_does_not_allow_a_late_prepared_command()
{
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let lease = open(&runtime);
    let first = command(lease, 1);
    let late = prepared(&runtime, &first).await;
    runtime
        .reject(
            first.clone(),
            serde_json::to_vec(&first).unwrap(),
            "first rejected",
        )
        .unwrap();
    for sequence in 2..=65 {
        let next = command(lease, sequence);
        runtime
            .reject(
                next.clone(),
                serde_json::to_vec(&next).unwrap(),
                "later rejection",
            )
            .unwrap();
    }
    assert_eq!(runtime.shared.lock().records.len(), 64);
    assert!(runtime.observe(first.identity).is_err());
    assert!(runtime.submit(late, authorization()).is_err());
    assert!(
        runtime
            .reject(
                first.clone(),
                serde_json::to_vec(&first).unwrap(),
                "retired uncertainty is not new evidence"
            )
            .is_err()
    );
    assert_eq!(servo.count(), 1);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn absent_old_lease_receipt_cannot_be_called_rejected_after_it_already_executed() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let old = command(open(&runtime), 1);
    assert_eq!(
        terminal(submitted(&runtime, &old).await, &old).await.end,
        Some(BodyEnd::Submitted)
    );
    let new_lease = open(&runtime);
    for sequence in 1..=64 {
        let next = command(new_lease, sequence);
        runtime
            .reject(
                next.clone(),
                serde_json::to_vec(&next).unwrap(),
                "fill bounded receipt cache",
            )
            .unwrap();
    }
    assert!(runtime.observe(old.identity).is_err());
    assert!(
        runtime
            .reject(
                old.clone(),
                serde_json::to_vec(&old).unwrap(),
                "old lease cache miss"
            )
            .is_err()
    );
    assert_eq!(servo.count(), 2);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn expired_authority_can_prove_unadmitted_identity_without_reviving_write_permission() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let lease = runtime
        .open_control(
            0,
            Uuid::new_v4(),
            BodyAuthorization {
                expires_at: Instant::now() + Duration::from_millis(100),
            },
        )
        .unwrap();
    let command = command(lease, 1);
    let pending = prepared(&runtime, &command).await;
    tokio::time::sleep(Duration::from_millis(125)).await;
    assert!(runtime.submit(pending, authorization()).is_err());
    let rejected = runtime
        .reject(
            command.clone(),
            serde_json::to_vec(&command).unwrap(),
            "lease expired before admission",
        )
        .unwrap()
        .snapshot();
    rejected.validate(None, &command).unwrap();
    assert!(matches!(rejected.end, Some(BodyEnd::Rejected { .. })));
    assert_eq!(rejected.accepted_actions, 0);
    assert!(runtime.renew_control(&lease, authorization()).is_err());
    assert_eq!(servo.count(), 1);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn invalid_or_unknown_lease_commands_cannot_fabricate_rejection_or_consume_current_waterline()
{
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let lease = open(&runtime);
    let valid = command(lease, 1);
    let mut invalid = valid.clone();
    invalid.actions.clear();
    assert!(
        runtime
            .reject(
                invalid.clone(),
                serde_json::to_vec(&invalid).unwrap(),
                "invalid structure"
            )
            .is_err()
    );
    let mut future = valid.clone();
    future.identity.lease.generation += 1;
    assert!(
        runtime
            .reject(
                future.clone(),
                serde_json::to_vec(&future).unwrap(),
                "future lease"
            )
            .is_err()
    );
    assert_eq!(
        terminal(submitted(&runtime, &valid).await, &valid)
            .await
            .end,
        Some(BodyEnd::Submitted)
    );
    assert_eq!(servo.count(), 2);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn replacing_control_cannot_overlap_native_output_or_let_old_close_revoke_new_owner() {
    let (runtime, servo) = unstarted();
    let runtime = Arc::new(runtime);
    runtime.start().await.unwrap();
    let old_lease = open(&runtime);
    let first = command(old_lease, 1);
    let _native_guard = servo.block_next();
    let old_observation = submitted(&runtime, &first).await;
    servo.wait_entered().await;
    let stale_generation = runtime.control_state().generation;
    let new_lease = open(&runtime);
    assert!(
        runtime
            .open_control(stale_generation, Uuid::new_v4(), authorization())
            .is_err()
    );
    assert!(
        runtime
            .close_control(&old_lease, "old delayed close")
            .is_err()
    );
    assert!(runtime.renew_control(&old_lease, authorization()).is_err());
    let second = command(new_lease, 1);
    let new_observation = submitted(&runtime, &second).await;
    let waiter = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.cancel(first.identity, "original only").await }
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(!old_observation.snapshot().writer_quiescent);
    assert_eq!(new_observation.snapshot().started_actions, 0);
    assert_eq!(servo.active.load(Ordering::Acquire), 1);
    servo.release();
    let old_end = terminal(old_observation, &first).await;
    assert!(matches!(old_end.end, Some(BodyEnd::Cancelled { .. })));
    assert_eq!(old_end.submitted_frames, 1);
    let new_end = terminal(new_observation, &second).await;
    assert_eq!(new_end.end, Some(BodyEnd::Submitted));
    assert_eq!(runtime.control_state().lease, Some(new_lease));
    assert_eq!(servo.max_active.load(Ordering::Acquire), 1);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn cancellation_deadline_keeps_native_owner_and_accepts_late_quiescent_checkpoint() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let command = command(open(&runtime), 1);
    let _native_guard = servo.block_next();
    let observation = submitted(&runtime, &command).await;
    servo.wait_entered().await;
    let error = runtime
        .cancel(command.identity, "deadline test")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("deadline exceeded"));
    assert!(!observation.snapshot().writer_quiescent);
    assert!(observation.snapshot().end.is_none());
    assert_eq!(servo.active.load(Ordering::Acquire), 1);
    servo.release();
    let end = terminal(observation, &command).await;
    assert!(matches!(end.end, Some(BodyEnd::Cancelled { .. })));
    assert_eq!(
        runtime
            .cancel(command.identity, "late acknowledgement")
            .await
            .unwrap(),
        end
    );
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn shutdown_waiter_drop_keeps_join_and_holds_after_entered_write_returns() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let command = command(open(&runtime), 1);
    let _native_guard = servo.block_next();
    let observation = submitted(&runtime, &command).await;
    servo.wait_entered().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(25), runtime.stop())
            .await
            .is_err()
    );
    assert_eq!(
        servo.count(),
        1,
        "shutdown has not entered another native frame"
    );
    assert!(!observation.snapshot().writer_quiescent);
    assert!(!runtime.control_state().available);
    assert!(
        runtime
            .open_control(
                runtime.control_state().generation,
                Uuid::new_v4(),
                authorization()
            )
            .is_err()
    );
    servo.release();
    runtime.stop().await.unwrap();
    let end = terminal(observation, &command).await;
    assert!(matches!(end.end, Some(BodyEnd::Cancelled { .. })));
    assert_eq!(
        servo.count(),
        2,
        "startup and old entered frame only; stop must hold"
    );
    assert_eq!(servo.max_active.load(Ordering::Acquire), 1);
    runtime.stop().await.unwrap();
    assert_eq!(servo.count(), 2, "stop is idempotent and emits no reset");
}

#[tokio::test]
async fn unconfirmed_calibration_startup_does_not_submit_native_targets() {
    let (mut runtime, servo) = unstarted();
    Arc::make_mut(&mut runtime.config).calibration_confirmed = false;
    runtime.start().await.unwrap();
    let available = runtime.control_state().available;
    runtime.stop().await.unwrap();
    assert_eq!(
        servo.count(),
        0,
        "uncalibrated startup must not send a center target"
    );
    assert!(!available, "uncalibrated writer must not admit motion");
}

#[tokio::test]
async fn unconfirmed_native_calibration_is_rejected_before_construction_or_start() {
    let config = MotionConfig {
        hardware_enabled: true,
        neck_config_file: "/definitely-absent-r03-calibration".into(),
        ..MotionConfig::default()
    };
    let error = MotionRuntime::new(config).err().unwrap().to_string();
    assert!(error.contains("requires confirmed mechanism calibration"));

    let (mut runtime, servo) = unstarted();
    let config = Arc::make_mut(&mut runtime.config);
    config.hardware_enabled = true;
    config.calibration_confirmed = false;
    let error = runtime.start().await.unwrap_err().to_string();
    runtime.stop().await.unwrap();
    assert!(error.contains("requires confirmed mechanism calibration"));
    assert_eq!(servo.count(), 0);
    assert!(!runtime.control_state().available);
}

#[tokio::test]
async fn unconfirmed_mock_does_not_load_a_matrix_or_advertise_motion() {
    let config = MotionConfig {
        neck_config_file: "/definitely-absent-r03-calibration".into(),
        ..MotionConfig::default()
    };
    let runtime = MotionRuntime::new(config).unwrap();
    runtime.start().await.unwrap();
    assert!(runtime.mapper.is_none());
    assert!(!runtime.shared.lock().spawned);
    assert!(!runtime.control_state().available);
    let mut metadata = DiscoveryMetadata {
        status: "test".into(),
        server_id: None,
        capabilities: vec![],
        attributes: Default::default(),
    };
    runtime.enrich_discovery_metadata(&mut metadata);
    assert!(
        !metadata
            .capabilities
            .iter()
            .any(|value| value == "motion.native")
    );
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn confirmed_startup_waits_for_one_complete_center_submission_before_admission() {
    let (runtime, servo) = unstarted();
    let runtime = Arc::new(runtime);
    let _native_guard = servo.block_next();
    let start = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.start().await }
    });
    servo.wait_entered().await;
    assert!(!runtime.control_state().available);
    assert!(
        runtime
            .open_control(0, Uuid::new_v4(), authorization())
            .is_err()
    );
    assert_eq!(servo.count(), 0);
    servo.release();
    start.await.unwrap().unwrap();
    assert!(runtime.control_state().available);
    assert_eq!(
        servo.count(),
        1,
        "initial angle is unknown; submit center once"
    );
    assert_eq!(servo.writes.lock().unwrap()[0].1, vec![90.0; 3]);
    runtime.stop().await.unwrap();
    assert_eq!(servo.count(), 1, "stop keeps the submitted center target");
}

#[tokio::test]
async fn partial_startup_never_admits_commands_or_retries_the_center_target() {
    let (runtime, servo) = unstarted();
    servo.partial.store(true, Ordering::Release);
    assert!(runtime.start().await.is_err());
    assert!(!runtime.control_state().available);
    assert!(runtime.stop().await.is_err());
    assert!(runtime.start().await.is_err());
    assert_eq!(servo.count(), 1);
}

#[test]
fn confirmed_calibration_still_requires_matching_dimensions_and_valid_center() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("neck.json");
    std::fs::write(&path, CALIBRATION.replace("[0,180]", "[40,80]")).unwrap();
    let mut config = MotionConfig {
        calibration_confirmed: true,
        neck_config_file: path.clone(),
        servo_count: 3,
        ..MotionConfig::default()
    };
    let error = MotionRuntime::new(config.clone())
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("exceeds calibrated limits"));
    config.center_angle = 60.0;
    config.servo_count = 4;
    let error = MotionRuntime::new(config.clone())
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("must match the calibration output count"));
    config.servo_count = 3;
    std::fs::write(&path, "not valid calibration JSON").unwrap();
    assert!(MotionRuntime::new(config).is_err());
}

#[tokio::test]
async fn lease_close_and_binding_revocation_hold_after_the_entered_write() {
    for binding_revocation in [false, true] {
        let (runtime, servo) = unstarted();
        runtime.start().await.unwrap();
        let lease = open(&runtime);
        let command = command(lease, 1);
        let _native_guard = servo.block_next();
        let observation = submitted(&runtime, &command).await;
        servo.wait_entered().await;
        if binding_revocation {
            runtime.revoke_control("device binding changed").unwrap();
        } else {
            runtime
                .close_control(&lease, "control owner closed")
                .unwrap();
        }
        assert!(!observation.snapshot().writer_quiescent);
        assert!(observation.snapshot().end.is_none());
        assert!(runtime.control_state().lease.is_none());
        assert_eq!(servo.count(), 1);
        assert_eq!(servo.active.load(Ordering::Acquire), 1);
        servo.release();
        let end = terminal(observation, &command).await;
        assert!(matches!(end.end, Some(BodyEnd::Cancelled { .. })));
        assert!(end.writer_quiescent);
        assert_eq!(end.submitted_frames, 1);
        assert_eq!(end.position_confirmed_actions, None);
        runtime.stop().await.unwrap();
        assert_eq!(
            servo.count(),
            2,
            "startup plus entered write; no reset after revoke"
        );
        assert_eq!(servo.max_active.load(Ordering::Acquire), 1);
    }
}

#[tokio::test]
async fn dropping_observer_keeps_the_native_owner_and_current_control_lease() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let lease = open(&runtime);
    let command = command(lease, 1);
    let _native_guard = servo.block_next();
    let observation = submitted(&runtime, &command).await;
    servo.wait_entered().await;
    drop(observation);
    assert_eq!(runtime.control_state().lease, Some(lease));
    assert_eq!(servo.active.load(Ordering::Acquire), 1);
    let observation = runtime.observe(command.identity).unwrap();
    assert!(!observation.snapshot().writer_quiescent);
    assert!(observation.snapshot().end.is_none());
    servo.release();
    let end = terminal(observation, &command).await;
    assert_eq!(end.end, Some(BodyEnd::Submitted));
    assert_eq!(runtime.control_state().lease, Some(lease));
    runtime.stop().await.unwrap();
    assert_eq!(
        servo.count(),
        2,
        "dropping observation adds no cancellation or reset"
    );
}

#[tokio::test]
async fn dropping_start_waiter_does_not_lose_the_entered_native_initialization_owner() {
    let (runtime, servo) = unstarted();
    let runtime = Arc::new(runtime);
    let _native_guard = servo.block_next();
    let start = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.start().await }
    });
    servo.wait_entered().await;
    start.abort();
    assert!(start.await.unwrap_err().is_cancelled());
    assert!(!runtime.control_state().available);
    assert!(
        tokio::time::timeout(Duration::from_millis(25), runtime.stop())
            .await
            .is_err()
    );
    assert_eq!(servo.active.load(Ordering::Acquire), 1);
    servo.release();
    runtime.stop().await.unwrap();
    assert_eq!(servo.count(), 1);
    assert!(runtime.start().await.is_err());
}

#[tokio::test]
async fn shutdown_join_error_is_sticky_and_a_completed_writer_handle_is_not_polled_twice() {
    let (runtime, servo) = unstarted();
    runtime.shared.lock().spawned = true;
    *runtime.writer.lock().await = Some(tokio::spawn(async {
        panic!("injected owner-level join failure")
    }));
    let first = runtime.stop().await.unwrap_err().to_string();
    assert!(first.contains("joining owned body writer"));
    assert!(runtime.writer.lock().await.is_none());
    assert_eq!(runtime.stop().await.unwrap_err().to_string(), first);
    assert_eq!(servo.count(), 0);
}

#[tokio::test]
async fn preparation_join_error_does_not_skip_joining_the_native_writer() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    runtime.preparations.lock().await.push(tokio::spawn(async {
        panic!("injected preparation owner failure")
    }));
    let first = runtime.stop().await.unwrap_err().to_string();
    assert!(first.contains("joining body preparation"));
    assert!(runtime.preparations.lock().await.is_empty());
    assert!(runtime.writer.lock().await.is_none());
    assert_eq!(runtime.stop().await.unwrap_err().to_string(), first);
    assert_eq!(servo.count(), 1);
}

#[tokio::test]
async fn dropping_library_runtime_seals_admission_and_finishes_its_existing_writer() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let command = command(open(&runtime), 1);
    let _native_guard = servo.block_next();
    let observation = submitted(&runtime, &command).await;
    servo.wait_entered().await;
    drop(runtime);
    assert!(!observation.snapshot().writer_quiescent);
    servo.release();
    let end = terminal(observation, &command).await;
    assert!(matches!(end.end, Some(BodyEnd::Cancelled { .. })));
    assert_eq!(servo.max_active.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn expired_authority_cannot_be_renewed_to_revive_an_entered_command() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let lease = runtime
        .open_control(
            0,
            Uuid::new_v4(),
            BodyAuthorization {
                expires_at: Instant::now() + Duration::from_millis(300),
            },
        )
        .unwrap();
    let mut command = command(lease, 1);
    command.actions[0].duration_ms = 100;
    let prepared = prepared(&runtime, &command).await;
    let _native_guard = servo.block_next();
    let observation = runtime
        .submit(
            prepared,
            BodyAuthorization {
                expires_at: Instant::now() + Duration::from_millis(40),
            },
        )
        .unwrap();
    servo.wait_entered().await;
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(runtime.renew_control(&lease, authorization()).is_err());
    assert!(runtime.control_state().lease.is_none());
    servo.release();
    let end = terminal(observation, &command).await;
    assert!(matches!(end.end, Some(BodyEnd::Cancelled { .. })));
    assert_eq!(end.submitted_frames, 1);
    assert_eq!(servo.count(), 2);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn voice_epoch_fence_preserves_old_checkpoint_without_cancelling_new_epoch() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let lease = open(&runtime);
    let old_scope = VoiceScope {
        device_boot_id: lease.device_boot_id,
        session_id: Uuid::new_v4(),
        generation_epoch: 1,
    };
    runtime.activate_voice(&old_scope).unwrap();
    let mut old = command(lease, 1);
    old.scope = BodyScope::Voice { scope: old_scope };
    let _native_guard = servo.block_next();
    let old_observation = submitted(&runtime, &old).await;
    servo.wait_entered().await;
    let new_scope = VoiceScope {
        generation_epoch: 2,
        ..old_scope
    };
    runtime.fence_voice(&new_scope).unwrap();
    let mut stale = command(lease, 2);
    stale.scope = old.scope;
    assert!(
        runtime
            .submit(prepared(&runtime, &stale).await, authorization())
            .is_err()
    );
    let mut new = command(lease, 3);
    new.scope = BodyScope::Voice { scope: new_scope };
    let new_observation = submitted(&runtime, &new).await;
    runtime.close_voice(&old_scope).unwrap();
    runtime.fence_voice(&old_scope).unwrap();
    assert_eq!(new_observation.snapshot().started_actions, 0);
    servo.release();
    assert!(matches!(
        terminal(old_observation, &old).await.end,
        Some(BodyEnd::Cancelled { .. })
    ));
    assert_eq!(
        terminal(new_observation, &new).await.end,
        Some(BodyEnd::Submitted)
    );
    runtime.close_voice(&new_scope).unwrap();
    let mut closed = command(lease, 4);
    closed.scope = new.scope;
    assert!(
        runtime
            .submit(prepared(&runtime, &closed).await, authorization())
            .is_err()
    );
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn queued_cancellation_is_immediately_quiescent_even_when_another_native_call_is_stuck() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let lease = open(&runtime);
    let first = command(lease, 1);
    let _native_guard = servo.block_next();
    let first_observation = submitted(&runtime, &first).await;
    servo.wait_entered().await;
    let second = command(lease, 2);
    let second_observation = submitted(&runtime, &second).await;
    let second_end = runtime
        .cancel(second.identity, "cancel queued command only")
        .await
        .unwrap();
    assert!(second_end.writer_quiescent);
    assert_eq!(second_end.submitted_frames, 0);
    assert_eq!(second_end.started_actions, 0);
    assert!(!first_observation.snapshot().writer_quiescent);
    assert_eq!(second_observation.snapshot(), second_end);
    servo.release();
    assert_eq!(
        terminal(first_observation, &first).await.end,
        Some(BodyEnd::Submitted)
    );
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn native_partial_failure_preserves_complete_and_uncertain_channels_and_seals_writer() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let command = command(open(&runtime), 1);
    servo.partial.store(true, Ordering::Release);
    let end = terminal(submitted(&runtime, &command).await, &command).await;
    assert!(matches!(end.end, Some(BodyEnd::Failed { .. })));
    assert_eq!(end.submitted_frames, 0);
    assert_eq!(end.partial_frame, Some(0));
    assert_eq!(end.last_partial_channels, vec![0]);
    assert_eq!(end.last_uncertain_channels, vec![1]);
    assert_eq!(end.position_confirmed_actions, None);
    assert!(runtime.stop().await.is_err());
    assert!(!runtime.control_state().available);
    assert_eq!(servo.count(), 2, "no reset using uncertain native targets");
}

#[tokio::test]
async fn native_panic_is_unknown_and_no_replacement_writer_is_spawned() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let command = command(open(&runtime), 1);
    servo.panic.store(true, Ordering::Release);
    let end = terminal(submitted(&runtime, &command).await, &command).await;
    assert!(matches!(end.end, Some(BodyEnd::Unknown { .. })));
    assert_eq!(
        runtime
            .reject(
                command.clone(),
                serde_json::to_vec(&command).unwrap(),
                "a retry error cannot replace native unknown"
            )
            .unwrap()
            .snapshot(),
        end
    );
    assert_eq!(
        end.submitted_frames, 0,
        "an unwound call supplied no frame evidence"
    );
    assert!(runtime.stop().await.is_err());
    assert!(runtime.start().await.is_err());
    assert_eq!(servo.max_active.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn entire_speed_expansion_is_checked_before_the_first_command_frame() {
    let (mut runtime, servo) = unstarted();
    Arc::make_mut(&mut runtime.config).max_servo_speed_deg_per_sec = 0.000_001;
    runtime.start().await.unwrap();
    let command = command(open(&runtime), 1);
    let end = terminal(submitted(&runtime, &command).await, &command).await;
    assert!(matches!(end.end, Some(BodyEnd::Rejected { .. })));
    assert_eq!(end.started_actions, 0);
    assert_eq!(end.submitted_frames, 0);
    assert_eq!(servo.count(), 1);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn queue_and_receipt_cache_are_bounded_and_retired_sequences_cannot_replay() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let lease = open(&runtime);
    let first = command(lease, 1);
    let _native_guard = servo.block_next();
    let first_observation = submitted(&runtime, &first).await;
    servo.wait_entered().await;
    for sequence in 2..=9 {
        submitted(&runtime, &command(lease, sequence)).await;
    }
    let overflowing = command(lease, 10);
    assert!(
        runtime
            .submit(prepared(&runtime, &overflowing).await, authorization())
            .is_err()
    );
    assert_eq!(runtime.shared.lock().queue.len(), 8);
    runtime.revoke_control("clear bounded queue").unwrap();
    servo.release();
    terminal(first_observation, &first).await;
    let lease = open(&runtime);
    let first_new = command(lease, 1);
    terminal(submitted(&runtime, &first_new).await, &first_new).await;
    for sequence in 2..=65 {
        let next = command(lease, sequence);
        terminal(submitted(&runtime, &next).await, &next).await;
    }
    assert_eq!(runtime.shared.lock().records.len(), 64);
    assert!(runtime.observe(first_new.identity).is_err());
    let before = servo.count();
    assert!(
        runtime
            .submit(prepared(&runtime, &first_new).await, authorization())
            .is_err()
    );
    assert_eq!(servo.count(), before);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn preparation_capacity_and_stop_ownership_survive_dropped_rpc_waiters() {
    let (mut runtime, _servo) = unstarted();
    let directory = tempfile::tempdir().unwrap();
    let bytes = b"frames\n1 0 0\n";
    std::fs::write(directory.path().join("reviewed.anim"), bytes).unwrap();
    let asset = alia_core::body::BodyAnimationAsset {
        file: "reviewed.anim".into(),
        version: 1,
        sha256: alia_core::body::body_animation_sha256(bytes),
        bytes: bytes.len() as u32,
    };
    Arc::make_mut(&mut runtime.config).animation_dir = directory.path().into();
    Arc::make_mut(&mut runtime.config).approved_animations = vec![asset.clone()];
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let _preparation_release = PreparationRelease(gate.clone());
    let entered = Arc::new(AtomicUsize::new(0));
    runtime.preparation_hook = Some(Arc::new({
        let gate = gate.clone();
        let entered = entered.clone();
        move || {
            entered.fetch_add(1, Ordering::AcqRel);
            let mut released = gate.0.lock().unwrap();
            while !*released {
                released = gate.1.wait(released).unwrap();
            }
        }
    }));
    let runtime = Arc::new(runtime);
    runtime.start().await.unwrap();
    let lease = open(&runtime);
    let mut waiters = Vec::new();
    for sequence in 1..=4 {
        let runtime = runtime.clone();
        let mut command = command(lease, sequence);
        command.actions[0].duration_ms = 100;
        command.actions[0].motion = BodyMotion::Animation {
            asset: asset.clone(),
            fps: 30,
            loops: 1,
        };
        waiters.push(tokio::spawn(async move {
            runtime
                .prepare(command.clone(), serde_json::to_vec(&command).unwrap())
                .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while entered.load(Ordering::Acquire) != 4 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    for waiter in waiters {
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
    }
    let fifth = command(lease, 5);
    assert!(
        runtime
            .prepare(fifth.clone(), serde_json::to_vec(&fifth).unwrap())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(25), runtime.stop())
            .await
            .is_err()
    );
    assert_eq!(runtime.preparations.lock().await.len(), 4);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    runtime.stop().await.unwrap();
    assert!(runtime.preparations.lock().await.is_empty());
}

#[tokio::test]
async fn prepared_approved_animation_cannot_revive_a_retired_identity_or_revoked_lease() {
    let (mut runtime, servo) = unstarted();
    let directory = tempfile::tempdir().unwrap();
    let bytes = b"frames\n1 0 0\n";
    let path = directory.path().join("reviewed.anim");
    std::fs::write(&path, bytes).unwrap();
    let asset = alia_core::body::BodyAnimationAsset {
        file: "reviewed.anim".into(),
        version: 1,
        sha256: alia_core::body::body_animation_sha256(bytes),
        bytes: bytes.len() as u32,
    };
    Arc::make_mut(&mut runtime.config).animation_dir = directory.path().into();
    Arc::make_mut(&mut runtime.config).approved_animations = vec![asset.clone()];
    runtime.start().await.unwrap();
    let lease = open(&runtime);
    let mut intent = command(lease, 1);
    intent.actions[0].duration_ms = 100;
    intent.actions[0].motion = BodyMotion::Animation {
        asset,
        fps: 30,
        loops: 1,
    };
    let late = prepared(&runtime, &intent).await;
    let rejected = runtime
        .reject(
            intent.clone(),
            serde_json::to_vec(&intent).unwrap(),
            "cancel before admission",
        )
        .unwrap()
        .snapshot();
    assert_eq!(
        runtime.submit(late, authorization()).unwrap().snapshot(),
        rejected
    );
    intent.identity.sequence += 1;
    let prepared_next = prepared(&runtime, &intent).await;
    std::fs::write(&path, b"frames\n7 0 0\n").unwrap();
    runtime
        .close_control(&lease, "lease revoked after preparation")
        .unwrap();
    assert!(runtime.submit(prepared_next, authorization()).is_err());
    assert_eq!(
        servo.count(),
        1,
        "only the confirmed startup target may be submitted"
    );
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn face_latest_slot_and_expression_use_one_writer() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    assert!(
        !runtime
            .track_face_center(50.0, 40.0, 100.0, 100.0)
            .await
            .unwrap()
    );
    let lease = open(&runtime);
    let _native_guard = servo.block_next();
    assert!(
        runtime
            .track_face_center(50.0, 40.0, 100.0, 100.0)
            .await
            .unwrap()
    );
    servo.wait_entered().await;
    assert!(
        runtime
            .track_face_center(45.0, 45.0, 100.0, 100.0)
            .await
            .unwrap()
    );
    assert!(
        runtime
            .track_face_center(40.0, 45.0, 100.0, 100.0)
            .await
            .unwrap()
    );
    assert!(runtime.shared.lock().face.is_some());
    let command = command(lease, 1);
    let observation = submitted(&runtime, &command).await;
    assert!(runtime.shared.lock().face.is_none());
    servo.release();
    assert_eq!(
        terminal(observation, &command).await.end,
        Some(BodyEnd::Submitted)
    );
    assert_eq!(servo.max_active.load(Ordering::Acquire), 1);
    runtime.revoke_control("binding revoked").unwrap();
    assert!(
        !runtime
            .track_face_center(50.0, 50.0, 100.0, 100.0)
            .await
            .unwrap()
    );
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn global_speed_clock_applies_across_consecutive_command_identities() {
    let (runtime, servo) = unstarted();
    runtime.start().await.unwrap();
    let lease = open(&runtime);
    for sequence in 1..=4 {
        let mut command = command(lease, sequence);
        command.actions[0].motion = BodyMotion::LookAt {
            target: BodyTarget::Rotation {
                degrees: [sequence as f32, 0.0, 0.0],
            },
        };
        terminal(submitted(&runtime, &command).await, &command).await;
    }
    let frames = servo.writes.lock().unwrap();
    for pair in frames.windows(2) {
        let elapsed = pair[1].0.duration_since(pair[0].0).as_secs_f32();
        for (old, new) in pair[0].1.iter().zip(&pair[1].1) {
            assert!(
                (new - old).abs() <= runtime.config.max_servo_speed_deg_per_sec * elapsed + 0.0001
            );
        }
    }
    drop(frames);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn rollback_before_start_does_not_move_hardware_and_disabled_control_is_unavailable() {
    let (runtime, servo) = unstarted();
    runtime.stop().await.unwrap();
    assert_eq!(servo.count(), 0);
    assert!(runtime.start().await.is_err());
    let disabled = MotionRuntime::disabled(MotionConfig::default());
    disabled.start().await.unwrap();
    assert!(!disabled.control_state().available);
    disabled.stop().await.unwrap();
}

#[test]
fn invalid_native_configuration_and_calibration_are_rejected_before_backend_construction() {
    let mut config = MotionConfig::default();
    config.fps = 0;
    assert!(validate_config(&config).is_err());
    config.fps = 30;
    config.max_servo_speed_deg_per_sec = f32::NAN;
    assert!(validate_config(&config).is_err());
    config.max_servo_speed_deg_per_sec = 60.0;
    config.face_smooth_alpha = f32::INFINITY;
    assert!(validate_config(&config).is_err());
    let bad_calibration = CALIBRATION.replace("\"sigma\": 20.0", "\"sigma\": 0.0");
    assert!(NeckMapper::from_json(&bad_calibration).is_err());
    let mapper = NeckMapper::from_json(CALIBRATION).unwrap();
    assert!(mapper.validate_angles(&[90.0, f32::NAN, 90.0]).is_err());
}
