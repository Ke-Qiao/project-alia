//! One admission gate and one native writer own every body output. Dropping an
//! RPC waiter cannot drop preparation, an admitted command, or a native write.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use alia_core::body::{
    BodyCommand, BodyCommandIdentity, BodyControlLease, BodyControlState, BodyEnd, BodyProgress,
    BodyScope, MAX_BODY_CHANNELS,
};
use alia_core::delivery::VoiceScope;
use anyhow::{Context, Result, bail, ensure};
use tokio::sync::{Mutex as AsyncMutex, oneshot, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::config::MotionConfig;
use crate::module::lifecycle::{ModuleHealth, ModuleLifecycle};
use crate::module::playback_session::VoiceOutputFence;
use crate::module::traits::DiscoveryMetadata;

use super::neck_mapper::NeckMapper;
use super::plan::{PreparedBodyCommand, face_rotation};
use super::servo::{ServoBackend, create_servo_backend};

const QUEUE_CAPACITY: usize = 8;
const RECEIPT_CAPACITY: usize = 64;
const PREPARATION_CAPACITY: usize = 4;
const MAX_AUTHORIZATION: Duration = Duration::from_secs(60);
const CANCEL_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
pub struct BodyAuthorization {
    /// Monotonic deadline derived from the verified signed request. It is
    /// checked again before each frame and each individual native write.
    pub expires_at: Instant,
}

impl BodyAuthorization {
    fn validate(self) -> Result<()> {
        let now = Instant::now();
        ensure!(self.expires_at > now, "body authorization has expired");
        ensure!(
            self.expires_at.duration_since(now) <= MAX_AUTHORIZATION,
            "body authorization exceeds 60 seconds"
        );
        Ok(())
    }
}

#[derive(Clone)]
pub struct BodyObservation {
    progress: watch::Receiver<BodyProgress>,
}

impl BodyObservation {
    pub fn snapshot(&self) -> BodyProgress {
        self.progress.borrow().clone()
    }

    pub async fn changed(&mut self) -> Result<()> {
        self.progress
            .changed()
            .await
            .context("body observation owner ended")
    }
}

struct AuthorityState {
    expires_at: Instant,
    revoked: Option<String>,
}

pub(super) struct Authority(Mutex<AuthorityState>);

impl Authority {
    fn new(authorization: BodyAuthorization) -> Self {
        Self(Mutex::new(AuthorityState {
            expires_at: authorization.expires_at,
            revoked: None,
        }))
    }

    fn reason(&self) -> Option<String> {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.revoked.is_none() && Instant::now() >= state.expires_at {
            state.revoked = Some("body authorization expired".into());
        }
        state.revoked.clone()
    }

    fn renew(&self, authorization: BodyAuthorization) -> Result<()> {
        authorization.validate()?;
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.revoked.is_none() && Instant::now() >= state.expires_at {
            state.revoked = Some("body authorization expired".into());
        }
        ensure!(
            state.revoked.is_none(),
            "body control lease is revoked or expired"
        );
        // A valid but earlier request cannot shorten another verified renewal.
        state.expires_at = state.expires_at.max(authorization.expires_at);
        Ok(())
    }

    fn revoke(&self, reason: &str) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        state.revoked.get_or_insert_with(|| reason.into());
    }
}

pub(super) struct Permit {
    authority: Arc<Authority>,
    cancelled: Mutex<Option<String>>,
    pub scope: Option<VoiceScope>,
    deadline: Option<Instant>,
}

impl Permit {
    fn new(authority: Arc<Authority>, scope: Option<VoiceScope>) -> Arc<Self> {
        Arc::new(Self {
            authority,
            cancelled: Mutex::new(None),
            scope,
            deadline: None,
        })
    }

    pub fn local() -> Arc<Self> {
        Self::new(
            Arc::new(Authority::new(BodyAuthorization {
                expires_at: Instant::now() + MAX_AUTHORIZATION,
            })),
            None,
        )
    }

    pub fn reason(&self) -> Option<String> {
        let mut cancelled = self
            .cancelled
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if cancelled.is_none() {
            *cancelled = self.authority.reason();
        }
        if cancelled.is_none() && self.deadline.is_some_and(|time| Instant::now() >= time) {
            *cancelled = Some("local face target expired".into());
        }
        cancelled.clone()
    }

    pub fn cancel(&self, reason: &str) {
        self.cancelled
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_or_insert_with(|| bounded_reason(reason));
    }
}

pub(super) struct CommandRecord {
    pub command: BodyCommand,
    canonical: Vec<u8>,
    pub permit: Arc<Permit>,
    current: Mutex<BodyProgress>,
    events: watch::Sender<BodyProgress>,
}

impl CommandRecord {
    fn new(
        command: BodyCommand,
        canonical: Vec<u8>,
        permit: Arc<Permit>,
        rejection: Option<String>,
    ) -> Arc<Self> {
        let progress = BodyProgress {
            identity: command.identity,
            receipt_sequence: 1,
            accepted_actions: if rejection.is_some() {
                0
            } else {
                command.actions.len() as u32
            },
            started_actions: 0,
            submitted_actions: 0,
            submitted_frames: 0,
            partial_frame: None,
            last_partial_channels: Vec::new(),
            last_uncertain_channels: Vec::new(),
            backend: None,
            writer_quiescent: rejection.is_some(),
            position_confirmed_actions: None,
            end: rejection.map(|reason| BodyEnd::Rejected { reason }),
        };
        let events = watch::channel(progress.clone()).0;
        Arc::new(Self {
            command,
            canonical,
            permit,
            current: Mutex::new(progress),
            events,
        })
    }

    pub fn observation(&self) -> BodyObservation {
        BodyObservation {
            progress: self.events.subscribe(),
        }
    }

    pub fn snapshot(&self) -> BodyProgress {
        self.current
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn update(&self, change: impl FnOnce(&mut BodyProgress)) -> Result<()> {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        ensure!(
            current.end.is_none(),
            "body observation is already terminal"
        );
        let mut next = current.clone();
        next.receipt_sequence = next
            .receipt_sequence
            .checked_add(1)
            .context("body receipt sequence exhausted")?;
        change(&mut next);
        next.validate(Some(&current), &self.command)
            .map_err(anyhow::Error::msg)?;
        *current = next.clone();
        self.events.send_replace(next);
        Ok(())
    }

    pub fn finish(&self, end: BodyEnd) -> Result<()> {
        self.update(|progress| {
            progress.writer_quiescent = true;
            progress.end = Some(end);
        })
    }
}

pub(super) struct QueuedCommand {
    pub prepared: PreparedBodyCommand,
    pub record: Arc<CommandRecord>,
}

pub(super) struct FaceWork {
    pub rotation: [f32; 3],
    pub permit: Arc<Permit>,
}

pub(super) enum ActiveWork {
    Command(Arc<CommandRecord>),
    Face(Arc<Permit>),
    Startup(Arc<Permit>),
}

impl ActiveWork {
    fn permit(&self) -> &Arc<Permit> {
        match self {
            Self::Command(record) => &record.permit,
            Self::Face(permit) | Self::Startup(permit) => permit,
        }
    }
}

struct LeaseState {
    identity: BodyControlLease,
    authority: Arc<Authority>,
    high_water: u64,
}

pub(super) struct State {
    pub boot: Uuid,
    generation: u64,
    lease: Option<LeaseState>,
    voice: Option<VoiceScope>,
    pub queue: VecDeque<QueuedCommand>,
    pub face: Option<FaceWork>,
    records: HashMap<BodyCommandIdentity, Arc<CommandRecord>>,
    receipts: VecDeque<BodyCommandIdentity>,
    pub active: Option<ActiveWork>,
    smoothed_face: [f32; 3],
    pub spawned: bool,
    pub ready: bool,
    pub stopping: bool,
    pub finished: bool,
    pub fault: Option<String>,
}

impl State {
    fn new(boot: Uuid) -> Self {
        Self {
            boot,
            generation: 0,
            lease: None,
            voice: None,
            queue: VecDeque::new(),
            face: None,
            records: HashMap::new(),
            receipts: VecDeque::new(),
            active: None,
            smoothed_face: [0.0; 3],
            spawned: false,
            ready: false,
            stopping: false,
            finished: false,
            fault: None,
        }
    }

    fn available(&self) -> bool {
        self.ready && !self.stopping && self.fault.is_none()
    }

    fn next_generation(&mut self) -> Result<u64> {
        self.generation = self
            .generation
            .checked_add(1)
            .context("body control generation exhausted")?;
        Ok(self.generation)
    }

    fn check_lease(&self, lease: &BodyControlLease) -> Result<&LeaseState> {
        let current = self
            .lease
            .as_ref()
            .context("body control lease is absent")?;
        ensure!(current.identity == *lease, "body control lease is stale");
        if let Some(reason) = current.authority.reason() {
            bail!(reason);
        }
        Ok(current)
    }

    /// This gate also serializes dequeue. A removed queued command has never
    /// entered native code and may immediately attest software quiescence.
    fn cancel_matching(
        &mut self,
        reason: &str,
        matches: impl Fn(&CommandRecord) -> bool,
    ) -> Result<()> {
        if let Some(ActiveWork::Command(record)) = &self.active
            && matches(record)
        {
            record.permit.cancel(reason);
        }
        let mut kept = VecDeque::with_capacity(QUEUE_CAPACITY);
        while let Some(queued) = self.queue.pop_front() {
            if matches(&queued.record) {
                queued.record.permit.cancel(reason);
                queued.record.finish(BodyEnd::Cancelled {
                    reason: bounded_reason(reason),
                })?;
            } else {
                kept.push_back(queued);
            }
        }
        self.queue = kept;
        Ok(())
    }

    fn cancel_face(&mut self, reason: &str) {
        self.face = None;
        if let Some(ActiveWork::Face(permit)) = &self.active {
            permit.cancel(reason);
        }
    }

    fn revoke(&mut self, reason: &str) -> Result<()> {
        if let Some(lease) = self.lease.take() {
            lease.authority.revoke(reason);
        }
        // Invalidate previously inspected generations even if no lease was open.
        self.next_generation()?;
        self.cancel_face(reason);
        self.cancel_matching(reason, |_| true)
    }

    fn retire_receipts(&mut self) -> Result<()> {
        while self.records.len() >= RECEIPT_CAPACITY {
            let position = self
                .receipts
                .iter()
                .position(|identity| {
                    self.records
                        .get(identity)
                        .is_some_and(|record| record.snapshot().end.is_some())
                })
                .context("body receipt capacity is exhausted")?;
            let identity = self.receipts.remove(position).expect("selected receipt");
            self.records.remove(&identity);
        }
        Ok(())
    }
}

pub(super) struct Shared {
    state: Mutex<State>,
    pub wake: Condvar,
    pub startup: watch::Sender<Option<std::result::Result<(), String>>>,
}

impl Shared {
    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn writer_finished(&self, result: &Result<()>) {
        let mut state = self.lock();
        state.ready = false;
        state.finished = true;
        if let Err(error) = result {
            state.fault.get_or_insert_with(|| format!("{error:#}"));
        }
        let reason = state
            .fault
            .clone()
            .unwrap_or_else(|| "body writer stopped".into());
        if let Some(lease) = state.lease.take() {
            lease.authority.revoke(&reason);
        }
        let active_identity = match &state.active {
            Some(ActiveWork::Command(record)) => Some(record.command.identity),
            _ => None,
        };
        for record in state.records.values() {
            if record.snapshot().end.is_none() {
                // A panic supplies no facts about the entered frame. Keep the
                // previous complete prefix and report the evidence gap.
                let end = if active_identity == Some(record.command.identity) {
                    BodyEnd::Unknown {
                        reason: reason.clone(),
                    }
                } else {
                    BodyEnd::Failed {
                        reason: reason.clone(),
                    }
                };
                let _ = record.finish(end);
            }
        }
        state.active = None;
        state.queue.clear();
        state.face = None;
        if self.startup.borrow().is_none() {
            self.startup.send_replace(Some(Err(reason)));
        }
        self.wake.notify_all();
    }
}

pub struct MotionRuntime {
    config: Arc<MotionConfig>,
    mapper: Option<NeckMapper>,
    servo: Arc<dyn ServoBackend>,
    shared: Arc<Shared>,
    // Await the handle by mutable reference. Cancelling start/stop waiters leaves
    // ownership in the instance for the next shutdown observer.
    writer: AsyncMutex<Option<JoinHandle<Result<()>>>>,
    preparations: AsyncMutex<Vec<JoinHandle<()>>>,
    #[cfg(test)]
    preparation_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl MotionRuntime {
    pub fn new(config: MotionConfig) -> Result<Self> {
        Self::new_with_boot(config, Uuid::new_v4())
    }

    pub fn new_with_boot(config: MotionConfig, boot: Uuid) -> Result<Self> {
        ensure!(!boot.is_nil(), "body device boot ID must not be nil");
        alia_core::body::validate_animation_catalog(&config.approved_animations)
            .map_err(anyhow::Error::msg)?;
        if !config.enabled {
            return Ok(Self::disabled_with_boot(config, boot));
        }
        validate_config(&config)?;
        // A matrix's presence is not confirmation of calibration on the device.
        // Unconfirmed simulation remains unavailable without loading demo data.
        let mapper = if config.calibration_confirmed {
            let mapper = NeckMapper::load(&config.neck_config_file)?;
            ensure!(
                mapper.servo_count() == config.servo_count,
                "servo count must match the calibration output count"
            );
            mapper.validate_angles(&vec![config.center_angle; config.servo_count])?;
            Some(mapper)
        } else {
            None
        };
        let servo = create_servo_backend(&config)?.into();
        Ok(Self::from_parts(config, boot, mapper, servo))
    }

    pub fn disabled(config: MotionConfig) -> Self {
        Self::disabled_with_boot(config, Uuid::new_v4())
    }

    pub fn disabled_with_boot(config: MotionConfig, boot: Uuid) -> Self {
        let config = MotionConfig {
            enabled: false,
            hardware_enabled: false,
            ..config
        };
        let servo = create_servo_backend(&config)
            .expect("disabled mock servo backend")
            .into();
        Self::from_parts(config, boot, None, servo)
    }

    fn from_parts(
        config: MotionConfig,
        boot: Uuid,
        mapper: Option<NeckMapper>,
        servo: Arc<dyn ServoBackend>,
    ) -> Self {
        Self {
            config: Arc::new(config),
            mapper,
            servo,
            shared: Arc::new(Shared {
                state: Mutex::new(State::new(boot)),
                wake: Condvar::new(),
                startup: watch::channel(None).0,
            }),
            writer: AsyncMutex::new(None),
            preparations: AsyncMutex::new(Vec::with_capacity(PREPARATION_CAPACITY)),
            #[cfg(test)]
            preparation_hook: None,
        }
    }

    pub fn control_state(&self) -> BodyControlState {
        let state = self.shared.lock();
        BodyControlState {
            device_boot_id: state.boot,
            generation: state.generation,
            lease: state
                .lease
                .as_ref()
                .filter(|lease| lease.authority.reason().is_none())
                .map(|lease| lease.identity),
            available: state.available(),
        }
    }

    pub fn open_control(
        &self,
        expected_generation: u64,
        server_instance_id: Uuid,
        authorization: BodyAuthorization,
    ) -> Result<BodyControlLease> {
        authorization.validate()?;
        ensure!(!server_instance_id.is_nil(), "nil body server instance");
        let mut state = self.shared.lock();
        ensure!(state.available(), "body writer is unavailable");
        ensure!(
            state.generation == expected_generation,
            "body control generation changed"
        );
        state.revoke("body control lease replaced")?;
        let identity = BodyControlLease {
            device_boot_id: state.boot,
            server_instance_id,
            generation: state.generation,
        };
        state.lease = Some(LeaseState {
            identity,
            authority: Arc::new(Authority::new(authorization)),
            high_water: 0,
        });
        self.shared.wake.notify_all();
        Ok(identity)
    }

    pub fn renew_control(
        &self,
        lease: &BodyControlLease,
        authorization: BodyAuthorization,
    ) -> Result<()> {
        let state = self.shared.lock();
        ensure!(state.available(), "body writer is unavailable");
        state.check_lease(lease)?.authority.renew(authorization)
    }

    pub fn close_control(&self, lease: &BodyControlLease, reason: &str) -> Result<()> {
        let mut state = self.shared.lock();
        // Expired owners may still close their exact current lease. An old
        // generation must never revoke a replacement owner's output.
        ensure!(
            state
                .lease
                .as_ref()
                .is_some_and(|current| current.identity == *lease),
            "body control lease is stale"
        );
        state.revoke(reason)?;
        self.shared.wake.notify_all();
        Ok(())
    }

    pub fn revoke_control(&self, reason: &str) -> Result<()> {
        self.shared.lock().revoke(reason)?;
        self.shared.wake.notify_all();
        Ok(())
    }

    pub async fn prepare(
        &self,
        command: BodyCommand,
        canonical_command_bytes: Vec<u8>,
    ) -> Result<PreparedBodyCommand> {
        let result_rx = {
            let mut preparations = self.preparations.lock().await;
            // Finished jobs have left their blocking closure; pending jobs retain
            // both capacity and an owned join handle after their RPC is dropped.
            let mut index = 0;
            while index < preparations.len() {
                if preparations[index].is_finished() {
                    preparations
                        .swap_remove(index)
                        .await
                        .context("body preparation worker panicked")?;
                } else {
                    index += 1;
                }
            }
            ensure!(
                preparations.len() < PREPARATION_CAPACITY,
                "body preparation capacity is exhausted"
            );
            let state = self.shared.lock();
            ensure!(state.available(), "body writer is unavailable");
            let config = self.config.clone();
            #[cfg(test)]
            let preparation_hook = self.preparation_hook.clone();
            let (result_tx, result_rx) = oneshot::channel();
            preparations.push(tokio::task::spawn_blocking(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    #[cfg(test)]
                    if let Some(hook) = preparation_hook {
                        hook();
                    }
                    super::plan::prepare(&config, command, canonical_command_bytes)
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("body preparation panicked")));
                let _ = result_tx.send(result);
            }));
            result_rx
        };
        result_rx.await.context("body preparation owner ended")?
    }

    pub fn submit(
        &self,
        prepared: PreparedBodyCommand,
        authorization: BodyAuthorization,
    ) -> Result<BodyObservation> {
        authorization.validate()?;
        let identity = prepared.command.identity;
        let mut state = self.shared.lock();
        ensure!(state.available(), "body writer is unavailable");
        let current = state.check_lease(&identity.lease)?;
        current.authority.renew(authorization)?;
        if let Some(record) = state.records.get(&identity) {
            ensure!(
                record.canonical == prepared.signed_bytes,
                "body identity was reused with a different intent"
            );
            return Ok(record.observation());
        }
        ensure!(
            identity.sequence > current.high_water,
            "body command sequence is retired or stale"
        );
        if let BodyScope::Voice { scope } = prepared.command.scope {
            ensure!(
                state.voice == Some(scope),
                "body command has no matching local voice scope"
            );
        }
        ensure!(
            state.queue.len() < QUEUE_CAPACITY,
            "body command queue is full"
        );
        let authority = current.authority.clone();
        state.retire_receipts()?;
        let permit = Permit::new(authority, prepared.command.scope.voice());
        let record = CommandRecord::new(
            prepared.command.clone(),
            prepared.signed_bytes.clone(),
            permit,
            None,
        );
        if let Some(ActiveWork::Command(active)) = &state.active
            && active.command.scope == record.command.scope
            && active.command.owner_id == record.command.owner_id
            && active.command.interruptible
            && active.command.priority < record.command.priority
        {
            active
                .permit
                .cancel("preempted by higher priority in the original scope");
        }
        state.cancel_face("body command admitted");
        state
            .lease
            .as_mut()
            .expect("checked control lease")
            .high_water = identity.sequence;
        state.records.insert(identity, record.clone());
        state.receipts.push_back(identity);
        state.queue.push_back(QueuedCommand {
            prepared,
            record: record.clone(),
        });
        self.shared.wake.notify_all();
        Ok(record.observation())
    }

    /// Prove that this immutable identity has never been admitted. An absent
    /// retired receipt is not rejection evidence: it may have executed earlier.
    /// Advancing the same admission waterline also fences preparation that has
    /// already started but has not yet returned to its RPC caller.
    pub fn reject(
        &self,
        command: BodyCommand,
        canonical: Vec<u8>,
        reason: &str,
    ) -> Result<BodyObservation> {
        super::plan::validate_canonical(&command, &canonical)?;
        let identity = command.identity;
        let mut state = self.shared.lock();
        if let Some(record) = state.records.get(&identity) {
            ensure!(
                record.canonical == canonical,
                "body identity was reused with a different intent"
            );
            return Ok(record.observation());
        }
        let lease = state
            .lease
            .as_ref()
            .context("body rejection cannot be proven without the original current lease")?;
        ensure!(
            identity.lease == lease.identity,
            "body rejection for a retired or unknown lease cannot be proven"
        );
        ensure!(
            identity.sequence > lease.high_water,
            "body rejection for a retired sequence cannot be proven"
        );
        // Authority may already have expired. The identity and admission
        // waterline, not renewed write permission, prove this specific rejection.
        let permit = Permit::new(lease.authority.clone(), command.scope.voice());
        state.retire_receipts()?;
        let record = CommandRecord::new(command, canonical, permit, Some(bounded_reason(reason)));
        state
            .lease
            .as_mut()
            .expect("verified original control lease")
            .high_water = identity.sequence;
        state.receipts.push_back(identity);
        state.records.insert(identity, record.clone());
        Ok(record.observation())
    }

    pub fn observe(&self, identity: BodyCommandIdentity) -> Result<BodyObservation> {
        let state = self.shared.lock();
        ensure!(
            identity.is_valid() && identity.lease.device_boot_id == state.boot,
            "invalid original body identity"
        );
        Ok(state
            .records
            .get(&identity)
            .context("body observation is unavailable; submission must not be replayed")?
            .observation())
    }

    pub async fn cancel(
        &self,
        identity: BodyCommandIdentity,
        reason: &str,
    ) -> Result<BodyProgress> {
        let mut observation = {
            let mut state = self.shared.lock();
            let record = state
                .records
                .get(&identity)
                .context("body observation is unavailable; cancellation has no quiescence proof")?
                .clone();
            state.cancel_matching(reason, |candidate| candidate.command.identity == identity)?;
            self.shared.wake.notify_all();
            record.observation()
        };
        tokio::time::timeout(CANCEL_DEADLINE, async {
            loop {
                let progress = observation.snapshot();
                if progress.writer_quiescent && progress.end.is_some() {
                    return Ok(progress);
                }
                observation.changed().await?;
            }
        })
        .await
        .context("body cancellation deadline exceeded; native writer remains owned")?
    }

    pub fn activate_voice(&self, scope: &VoiceScope) -> Result<()> {
        let mut state = self.shared.lock();
        ensure!(
            scope.is_valid() && scope.device_boot_id == state.boot,
            "invalid local body voice scope"
        );
        if let Some(current) = state.voice
            && current.session_id == scope.session_id
        {
            ensure!(
                scope.generation_epoch >= current.generation_epoch,
                "body voice generation regressed"
            );
        }
        state.voice = Some(*scope);
        state.cancel_matching("local body voice scope replaced", |record| {
            record
                .command
                .scope
                .voice()
                .is_some_and(|old| old != *scope)
        })?;
        state.cancel_face("local body voice scope replaced");
        self.shared.wake.notify_all();
        Ok(())
    }

    pub fn fence_voice(&self, scope: &VoiceScope) -> Result<()> {
        let mut state = self.shared.lock();
        ensure!(
            scope.is_valid() && scope.device_boot_id == state.boot,
            "invalid local body voice scope"
        );
        let Some(current) = state.voice else {
            return Ok(());
        };
        if current.session_id != scope.session_id
            || scope.generation_epoch <= current.generation_epoch
        {
            return Ok(());
        }
        state.voice = Some(*scope);
        state.cancel_matching("local body voice generation fenced", |record| {
            record.command.scope.voice().is_some_and(|old| {
                old.session_id == scope.session_id && old.generation_epoch < scope.generation_epoch
            })
        })?;
        state.cancel_face("local body voice generation fenced");
        self.shared.wake.notify_all();
        Ok(())
    }

    pub fn close_voice(&self, scope: &VoiceScope) -> Result<()> {
        let mut state = self.shared.lock();
        ensure!(
            scope.is_valid() && scope.device_boot_id == state.boot,
            "invalid local body voice scope"
        );
        if state.voice != Some(*scope) {
            return Ok(());
        }
        state.voice = None;
        state.cancel_matching("local body voice scope closed", |record| {
            record.command.scope.voice() == Some(*scope)
        })?;
        state.cancel_face("local body voice scope closed");
        self.shared.wake.notify_all();
        Ok(())
    }

    pub async fn track_face_center(
        &self,
        cx: f32,
        cy: f32,
        width: f32,
        height: f32,
    ) -> Result<bool> {
        if !self.config.enabled {
            return Ok(false);
        }
        let rotation = face_rotation(&self.config, cx, cy, width, height)?;
        let mut state = self.shared.lock();
        if !state.available()
            || !state.queue.is_empty()
            || matches!(
                state.active,
                Some(ActiveWork::Command(_) | ActiveWork::Startup(_))
            )
        {
            return Ok(false);
        }
        let Some(lease) = &state.lease else {
            return Ok(false);
        };
        if lease.authority.reason().is_some() {
            return Ok(false);
        }
        let authority = lease.authority.clone();
        let alpha = self.config.face_smooth_alpha;
        for (previous, value) in state.smoothed_face.iter_mut().zip(rotation) {
            *previous = *previous * (1.0 - alpha) + value * alpha;
        }
        state.cancel_face("newer local face target");
        state.face = Some(FaceWork {
            rotation: state.smoothed_face,
            permit: Arc::new(Permit {
                authority,
                cancelled: Mutex::new(None),
                scope: state.voice,
                deadline: Some(Instant::now() + Duration::from_millis(200)),
            }),
        });
        self.shared.wake.notify_all();
        Ok(true)
    }

    pub fn enrich_discovery_metadata(&self, metadata: &mut DiscoveryMetadata) {
        // Persistent BodyDelivery must opt into the current Body contract.
        // A native writer alone must never advertise end-to-end completion.
        if self.config.enabled && self.config.calibration_confirmed && self.mapper.is_some() {
            metadata.capabilities.push("motion.native".into());
        }
        metadata.capabilities.sort();
        metadata.capabilities.dedup();
    }
}

impl VoiceOutputFence for MotionRuntime {
    fn activate_voice(&self, scope: &VoiceScope) -> Result<()> {
        MotionRuntime::activate_voice(self, scope)
    }
    fn fence_voice(&self, scope: &VoiceScope) -> Result<()> {
        MotionRuntime::fence_voice(self, scope)
    }
    fn close_voice(&self, scope: &VoiceScope) -> Result<()> {
        MotionRuntime::close_voice(self, scope)
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for MotionRuntime {
    fn name(&self) -> &'static str {
        "motion"
    }

    async fn start(&self) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        validate_config(&self.config)?;
        if !self.config.calibration_confirmed {
            return Ok(());
        }
        {
            let mut writer = self.writer.lock().await;
            let mut state = self.shared.lock();
            ensure!(
                !state.stopping && !state.finished,
                "body runtime cannot restart after shutdown"
            );
            if !state.spawned {
                state.spawned = true;
                let shared = self.shared.clone();
                let config = self.config.clone();
                let mapper = self
                    .mapper
                    .clone()
                    .context("body calibration is unavailable")?;
                let servo = self.servo.clone();
                *writer = Some(tokio::task::spawn_blocking(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        super::writer::run(&shared, &config, mapper, servo.clone())
                    }))
                    .unwrap_or_else(|_| {
                        Err(anyhow::anyhow!(
                            "owned body writer panicked; native evidence is unknown"
                        ))
                    });
                    let close =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| servo.close()))
                            .map_err(|_| anyhow::anyhow!("closing owned body backend panicked"));
                    let result = result.and(close);
                    shared.writer_finished(&result);
                    result
                }));
            }
        }
        let mut startup = self.shared.startup.subscribe();
        loop {
            if let Some(result) = startup.borrow_and_update().clone() {
                return result.map_err(anyhow::Error::msg);
            }
            startup
                .changed()
                .await
                .context("body startup owner ended")?;
        }
    }

    async fn stop(&self) -> Result<()> {
        {
            let mut state = self.shared.lock();
            if !state.stopping {
                state.stopping = true;
                state.ready = false;
                state.revoke("body runtime stopping")?;
                if let Some(active) = &state.active {
                    active.permit().cancel("body runtime stopping");
                }
                if !state.spawned {
                    state.finished = true;
                }
            }
            self.shared.wake.notify_all();
        }
        let mut preparations = self.preparations.lock().await;
        while let Some(job) = preparations.last_mut() {
            let result = job.await;
            preparations.pop();
            if let Err(error) = result {
                self.shared
                    .lock()
                    .fault
                    .get_or_insert_with(|| format!("joining body preparation: {error}"));
            }
        }
        drop(preparations);
        let mut writer = self.writer.lock().await;
        if let Some(job) = writer.as_mut() {
            let result = job
                .await
                .context("joining owned body writer")
                .and_then(|result| result);
            *writer = None;
            // Also handles a task-level panic outside the writer's native panic
            // boundary. Never leave a completed JoinHandle to be polled twice.
            if result.is_err() {
                self.shared.writer_finished(&result);
            }
        }
        if let Some(error) = self.shared.lock().fault.clone() {
            bail!(error);
        }
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let state = self.shared.lock();
        if state.fault.is_some() {
            ModuleHealth::Unhealthy
        } else if state.available() {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Degraded
        }
    }
}

impl Drop for MotionRuntime {
    fn drop(&mut self) {
        // The module cleanup normally awaits stop. A library host dropping its
        // last handle still seals admission and wakes the already owned worker.
        let mut state = self.shared.lock();
        state.stopping = true;
        state.ready = false;
        let _ = state.revoke("body runtime owner dropped");
        if let Some(active) = &state.active {
            active.permit().cancel("body runtime owner dropped");
        }
        self.shared.wake.notify_all();
    }
}

fn validate_config(config: &MotionConfig) -> Result<()> {
    ensure!(
        !config.hardware_enabled || config.calibration_confirmed,
        "native body output requires confirmed mechanism calibration"
    );
    ensure!(
        (1..=MAX_BODY_CHANNELS).contains(&config.servo_count),
        "body servo count must be 1..32"
    );
    ensure!(
        (1..=120).contains(&config.fps),
        "body writer FPS must be 1..120"
    );
    ensure!(
        config.max_servo_speed_deg_per_sec.is_finite()
            && config.max_servo_speed_deg_per_sec > 0.0
            && config.max_servo_speed_deg_per_sec <= 720.0,
        "body servo speed must be finite and 0..720 degrees/second"
    );
    ensure!(
        config.center_angle.is_finite() && (0.0..=180.0).contains(&config.center_angle),
        "invalid body center angle"
    );
    ensure!(
        config.serial_startup_delay_ms <= 30_000,
        "serial startup delay exceeds 30 seconds"
    );
    ensure!(
        [
            config.face_max_yaw,
            config.face_max_pitch,
            config.face_deadzone,
            config.face_smooth_alpha,
            config.face_center_offset_x,
            config.face_center_offset_y,
            config.face_camera_roll_deg
        ]
        .iter()
        .all(|value| value.is_finite()),
        "body face configuration must be finite"
    );
    ensure!(
        (0.0..=180.0).contains(&config.face_max_yaw)
            && (0.0..=180.0).contains(&config.face_max_pitch)
            && (0.0..=1.0).contains(&config.face_deadzone)
            && (0.0..=1.0).contains(&config.face_smooth_alpha),
        "invalid body face limits"
    );
    ensure!(
        config.min_pulse_us < config.max_pulse_us && config.pca_frequency_hz > 0,
        "invalid body PWM configuration"
    );
    if config.hardware_enabled && matches!(config.servo_backend.as_str(), "pca9685" | "i2c") {
        ensure!(
            config.pca9685_addresses.len() * 16 >= config.servo_count
                && config
                    .pca9685_addresses
                    .iter()
                    .all(|address| (0x03..=0x77).contains(address)),
            "PCA9685 board configuration does not cover channels"
        );
        let mut sorted = config.pca9685_addresses.clone();
        sorted.sort_unstable();
        sorted.dedup();
        ensure!(
            sorted.len() == config.pca9685_addresses.len(),
            "duplicate PCA9685 board addresses"
        );
    }
    Ok(())
}

pub(super) fn bounded_reason(reason: &str) -> String {
    reason.chars().take(256).collect()
}

#[cfg(test)]
mod tests;
