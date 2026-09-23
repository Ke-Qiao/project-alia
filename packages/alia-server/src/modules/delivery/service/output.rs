//! One supervised control owner for the original voice producer and all its
//! media deliveries. Control work never waits in the media admission queue.
mod gate;
use super::*;
use crate::modules::delivery::repository::control::{ControlAdmission, OutputControlStatus};
use alia_core::{
    delivery::VoiceScope,
    output_control::{BoundControlCommand, BoundControlReceipt, OutputBinding, OutputRegistration},
    playback_control::{
        CommandOutcome, ControlAction, ControlCommand, ControlPhase, HoldReason,
        MAX_CONTROL_COMMANDS, ResumeAuthorization,
    },
};
use anyhow::Context;
use gate::{DEFAULT_HOLD, Gate, Hold};
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

const CAPACITY: usize = 64;

enum Work {
    Bind(ResponseRecord, oneshot::Sender<anyhow::Result<()>>),
    Action {
        hold_id: Uuid,
        action: ControlAction,
    },
}

struct Inner {
    request_id: Uuid,
    owner: Uuid,
    scope: VoiceScope,
    channel: Arc<dyn ChannelAdapter>,
    cancel: CancellationToken,
    gate: StdMutex<Gate>,
    changed: watch::Sender<u64>,
    wake: Notify,
    registration: std::sync::OnceLock<OutputRegistration>,
    // Keep the original budget identity alive across sentence gaps and native
    // tails, even when no encoded buffer happens to be retained at that instant.
    audio_budget: std::sync::OnceLock<crate::providers::tts::audio_budget::ResponseAudioBudget>,
    sender: mpsc::Sender<Work>,
}

#[derive(Clone)]
pub struct VoiceOutput(Arc<Inner>);
impl std::fmt::Debug for VoiceOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceOutput")
            .field("scope", &self.0.scope)
            .field("request_id", &self.0.request_id)
            .finish()
    }
}

/// The last producer reference closes production, but never releases a hold or
/// drops a supervised media future. Native tail ownership is counted separately.
#[derive(Debug)]
pub struct OutputProducer {
    output: VoiceOutput,
}
impl Drop for OutputProducer {
    fn drop(&mut self) {
        self.output.0.gate.lock().unwrap().producer_finished = true;
        self.output.signal();
    }
}
impl OutputProducer {
    pub fn output(&self) -> VoiceOutput {
        self.output.clone()
    }
    pub fn request_id(&self) -> Uuid {
        self.output.0.request_id
    }
    pub fn scope(&self) -> VoiceScope {
        self.output.0.scope
    }
    pub fn owner(&self) -> Uuid {
        self.output.0.owner
    }
    pub fn channel(&self) -> Arc<dyn ChannelAdapter> {
        self.output.0.channel.clone()
    }
    pub async fn wait_open(&self) -> anyhow::Result<()> {
        self.output.wait_open().await
    }
    pub async fn bind(&self, response: ResponseRecord) -> anyhow::Result<()> {
        let (reply, receive) = oneshot::channel();
        self.output.enqueue(Work::Bind(response, reply))?;
        receive
            .await
            .context("original output control owner ended")?
    }
}

impl VoiceOutput {
    fn tighten_deadline(&self, deadline: chrono::DateTime<chrono::Utc>) {
        let now = Instant::now();
        let remaining = (deadline - chrono::Utc::now()).to_std().unwrap_or_default();
        {
            let mut gate = self.0.gate.lock().unwrap();
            gate.deadline = gate.deadline.min(now + remaining);
        }
        self.signal();
    }
    fn tighten_hold_deadline(&self, hold_id: Uuid, deadline: chrono::DateTime<chrono::Utc>) {
        let now = Instant::now();
        let remaining = (deadline - chrono::Utc::now()).to_std().unwrap_or_default();
        {
            let mut gate = self.0.gate.lock().unwrap();
            if let Some(hold) = gate.hold.as_mut().filter(|hold| hold.id == hold_id) {
                hold.deadline = hold.deadline.min(now + remaining);
            }
        }
        self.signal();
    }
    pub fn response_id(&self) -> Option<Uuid> {
        self.0
            .registration
            .get()
            .map(|registration| registration.binding.response_id)
    }
    fn signal(&self) {
        self.0
            .changed
            .send_modify(|version| *version = version.wrapping_add(1));
        self.0.wake.notify_one();
    }
    fn enqueue(&self, work: Work) -> anyhow::Result<()> {
        if self.0.sender.try_send(work).is_err() {
            self.retire();
            anyhow::bail!("original output control queue unavailable");
        }
        self.signal();
        Ok(())
    }
    pub fn is_alive(&self) -> bool {
        !self.0.cancel.is_cancelled() && self.0.gate.lock().unwrap().is_alive(Instant::now())
    }
    pub fn is_held(&self) -> bool {
        self.is_alive() && self.0.gate.lock().unwrap().hold.is_some()
    }
    pub fn hold_reason(&self) -> Option<HoldReason> {
        self.0
            .gate
            .lock()
            .unwrap()
            .hold
            .as_ref()
            .map(|hold| hold.reason)
    }
    fn pause_work(&self, hold: Hold) -> anyhow::Result<()> {
        // The first command carries a fixed remaining budget. Local deadlines
        // remain the stronger bound; retries cannot refresh either clock.
        let remaining = hold
            .deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u64;
        anyhow::ensure!(remaining > 0, "hold already expired");
        self.enqueue(Work::Action {
            hold_id: hold.id,
            action: ControlAction::Pause {
                reason: hold.reason,
                through_sequence: hold.through_sequence,
                budget_ms: remaining,
            },
        })
    }
    /// Called for every accepted near-end speech frame, before another model
    /// result may release the gate. It never treats VAD as semantic permission.
    pub fn near_end_speech(&self, sequence: u64) -> anyhow::Result<bool> {
        let hold = {
            let mut gate = self.0.gate.lock().unwrap();
            gate.last_speech = Some(gate.last_speech.map_or(sequence, |old| old.max(sequence)));
            gate.hold(HoldReason::AcousticCandidate, sequence, Instant::now())
        };
        if let Some(hold) = hold {
            self.signal();
            self.pause_work(hold)?;
            return Ok(true);
        }
        Ok(false)
    }
    pub fn explicit_wait(&self, detection_id: Uuid, through_sequence: u64) -> anyhow::Result<bool> {
        anyhow::ensure!(!detection_id.is_nil(), "wait has no fresh detection");
        let (hold, confirm) = {
            let mut gate = self.0.gate.lock().unwrap();
            let hold = gate.hold(HoldReason::ExplicitWait, through_sequence, Instant::now());
            let confirm = if hold.is_none() {
                gate.confirm(through_sequence, Instant::now())
            } else {
                None
            };
            (hold, confirm)
        };
        self.signal();
        if let Some(hold) = hold {
            self.pause_work(hold)?;
            return Ok(true);
        }
        if let Some(hold_id) = confirm {
            self.enqueue(Work::Action {
                hold_id,
                action: ControlAction::ConfirmExplicitWait {
                    detection_id,
                    through_sequence,
                },
            })?;
            return Ok(true);
        }
        Ok(false)
    }
    pub fn request_resume(&self, authorization: ResumeAuthorization) -> anyhow::Result<bool> {
        let hold_id = {
            let gate = self.0.gate.lock().unwrap();
            let Some(hold) = &gate.hold else {
                return Ok(false);
            };
            if !gate.can_resume(hold.id, authorization, Instant::now()) {
                return Ok(false);
            }
            hold.id
        };
        self.enqueue(Work::Action {
            hold_id,
            action: ControlAction::Resume { authorization },
        })?;
        Ok(true)
    }
    pub fn retire(&self) {
        self.0.gate.lock().unwrap().retired = true;
        self.0.cancel.cancel();
        self.signal();
    }
    pub async fn wait_open(&self) -> anyhow::Result<()> {
        let mut changed = self.0.changed.subscribe();
        loop {
            let (alive, held, deadline) = {
                let gate = self.0.gate.lock().unwrap();
                (
                    gate.is_alive(Instant::now()),
                    gate.hold.is_some(),
                    gate.deadline(),
                )
            };
            anyhow::ensure!(
                alive && !self.0.cancel.is_cancelled(),
                "original output ended or expired"
            );
            if !held {
                return Ok(());
            }
            tokio::select! { biased;
                _ = self.0.cancel.cancelled() => anyhow::bail!("original output cancelled"),
                result = changed.changed() => { result.context("output gate owner ended")?; },
                _ = tokio::time::sleep_until(deadline) => { self.retire(); anyhow::bail!("original hold expired"); },
            }
        }
    }
    pub(super) fn retain_media(&self, response_id: Uuid) -> anyhow::Result<MediaOwner> {
        let registration = self
            .0
            .registration
            .get()
            .context("output not registered before media admission")?;
        anyhow::ensure!(
            registration.binding.response_id == response_id,
            "media changed original response"
        );
        let mut gate = self.0.gate.lock().unwrap();
        anyhow::ensure!(
            gate.is_alive(Instant::now()) && !self.0.cancel.is_cancelled(),
            "original output no longer owns media"
        );
        gate.media_owners += 1;
        Ok(MediaOwner(self.clone()))
    }
    pub(super) fn gate_stream(&self, mut stream: AudioPayloadStream) -> AudioPayloadStream {
        use futures::StreamExt;
        let output = self.clone();
        Box::pin(async_stream::try_stream! {
            loop {
                output.wait_open().await?;
                let next = tokio::select! { biased;
                    _ = output.0.cancel.cancelled() => Err(anyhow::anyhow!("original output cancelled")),
                    next = stream.next() => Ok(next),
                }?;
                let Some(payload) = next else { break; };
                let payload = payload?;
                // A hold may have arrived while awaiting the bounded provider.
                // Retain that one payload and the original future; do not retry TTS.
                output.wait_open().await?;
                yield payload;
            }
        })
    }
}
pub(super) struct MediaOwner(VoiceOutput);
impl MediaOwner {
    pub(super) fn output(&self) -> &VoiceOutput {
        &self.0
    }
}
impl Drop for MediaOwner {
    fn drop(&mut self) {
        self.0.0.gate.lock().unwrap().media_owners -= 1;
        self.0.signal();
    }
}

pub(super) struct NewOutput {
    output: VoiceOutput,
    receiver: mpsc::Receiver<Work>,
}

impl DeliveryService {
    pub fn begin_voice_output(
        self: &Arc<Self>,
        owner: Uuid,
        request_id: Uuid,
        scope: VoiceScope,
        channel: Arc<dyn ChannelAdapter>,
        cancel: CancellationToken,
        hold_budget: Option<Duration>,
    ) -> anyhow::Result<Arc<OutputProducer>> {
        let _boundary = self.admission_boundary.lock().unwrap();
        self.ensure_open()?;
        anyhow::ensure!(
            self.running.load(Ordering::Acquire),
            "delivery supervision has not started"
        );
        anyhow::ensure!(
            scope.is_valid() && !request_id.is_nil() && channel.owner_user_id() == Some(owner),
            "invalid original output owner"
        );
        let mut outputs = self.outputs.lock().unwrap();
        anyhow::ensure!(
            outputs.len() < CAPACITY && !outputs.contains_key(&scope),
            "output scope already owned or capacity exhausted"
        );
        let gate = Gate::new(Instant::now(), hold_budget.unwrap_or(DEFAULT_HOLD))?;
        let (sender, receiver) = mpsc::channel(MAX_CONTROL_COMMANDS);
        let (changed, _) = watch::channel(0);
        let output = VoiceOutput(Arc::new(Inner {
            owner,
            request_id,
            scope,
            channel,
            cancel,
            gate: StdMutex::new(gate),
            changed,
            wake: Notify::new(),
            registration: std::sync::OnceLock::new(),
            audio_budget: std::sync::OnceLock::new(),
            sender,
        }));
        self.output_sender
            .try_send(NewOutput {
                output: output.clone(),
                receiver,
            })
            .map_err(|_| anyhow::anyhow!("output supervisor capacity exhausted"))?;
        outputs.insert(scope, output.clone());
        Ok(Arc::new(OutputProducer { output }))
    }

    pub(super) async fn supervise_output(self: Arc<Self>, mut input: NewOutput) {
        use futures::FutureExt;
        let output = input.output.clone();
        let result = std::panic::AssertUnwindSafe(self.run_output(&output, &mut input.receiver))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| {
                self.failures
                    .lock()
                    .unwrap()
                    .push("output control owner panicked".into());
                Err(anyhow::anyhow!("output control owner panicked"))
            });
        let finished = {
            let gate = output.0.gate.lock().unwrap();
            gate.producer_finished
                && gate.media_owners == 0
                && !gate.retired
                && !output.0.cancel.is_cancelled()
                && Instant::now() < gate.deadline()
        };
        output.retire();
        // Cancellation needs no permission to use the old Context. It closes
        // this exact generation and preserves whatever evidence already exists.
        let target = ChannelTarget {
            user_id: output.0.owner.to_string(),
            voice: Some(output.0.scope),
        };
        // Native acknowledgement and durable local retirement are independent.
        // A stuck native fence cannot hide a cancellation that is already known.
        let persist_retirement = async {
            if let Some(registration) = output.0.registration.get() {
                if !finished {
                    if let Err(error) = bounded(self.repository.request_cancel(
                        output.0.owner,
                        registration.binding.response_id,
                        "original voice output cancelled or expired",
                    ))
                    .await
                    {
                        self.failures
                            .lock()
                            .unwrap()
                            .push(format!("output response cancellation: {error}"));
                    }
                }
                if let Err(error) = self
                    .repository
                    .retire_control(
                        output.0.owner,
                        self.instance_id,
                        &registration.binding,
                        "original voice output ended",
                    )
                    .await
                {
                    self.failures
                        .lock()
                        .unwrap()
                        .push(format!("output control retirement: {error}"));
                }
            }
        };
        let fence_original = async {
            let fence = output
                .0
                .channel
                .stop_current_playback(&target, "original voice output ended");
            tokio::pin!(fence);
            let fence_result = match tokio::time::timeout(CANCEL_TIMEOUT, &mut fence).await {
                Ok(result) => result,
                Err(_) => {
                    self.failures
                        .lock()
                        .unwrap()
                        .push(format!("output fence deadline: {:?}", output.0.scope));
                    // The exact native operation still owns its channel and budget.
                    // A shutdown timeout must not silently detach that ownership.
                    fence.await
                }
            };
            if let Err(error) = fence_result {
                self.failures
                    .lock()
                    .unwrap()
                    .push(format!("output fence unconfirmed: {error:#}"));
            }
        };
        tokio::join!(persist_retirement, fence_original);
        self.outputs.lock().unwrap().remove(&output.0.scope);
        if let Err(error) = result {
            tracing::warn!(%error, scope=?output.0.scope, "original output retired");
        }
    }

    async fn run_output(
        &self,
        output: &VoiceOutput,
        receiver: &mut mpsc::Receiver<Work>,
    ) -> anyhow::Result<()> {
        let mut pending = std::collections::VecDeque::new();
        let mut sequence = 0u64;
        loop {
            let deadline = output.0.gate.lock().unwrap().deadline();
            if !output.is_alive() {
                return Ok(());
            }
            let work = if output.0.registration.get().is_some() && !pending.is_empty() {
                pending.pop_front()
            } else {
                tokio::select! { biased;
                    _ = self.stop.cancelled() => return Ok(()),
                    _ = output.0.cancel.cancelled() => return Ok(()),
                    _ = output.0.wake.notified() => continue,
                    _ = tokio::time::sleep_until(deadline) => anyhow::bail!("original output deadline"),
                    work = receiver.recv() => work,
                }
            };
            let Some(work) = work else {
                return Ok(());
            };
            if output.0.registration.get().is_none() {
                match work {
                    Work::Bind(response, reply) => {
                        let result = self
                            .output_operation(output, self.bind_output(output, &response))
                            .await;
                        if let Err(error) = result {
                            let _ = reply.send(Err(anyhow::anyhow!(error.to_string())));
                            return Err(error);
                        }
                        let _ = reply.send(Ok(()));
                    }
                    Work::Action {
                        hold_id,
                        action: ControlAction::Resume { authorization },
                    } => {
                        // Before canonical Response/registration there is no
                        // device output to resume. Only the retained producer is
                        // released; its later Context/Response admission remains
                        // mandatory. The voice actor revalidated original source.
                        if output.0.gate.lock().unwrap().resume(
                            hold_id,
                            authorization,
                            Instant::now(),
                        ) {
                            pending.clear();
                            output.signal();
                        }
                    }
                    other => {
                        anyhow::ensure!(
                            pending.len() < MAX_CONTROL_COMMANDS,
                            "pending controls exceeded capacity"
                        );
                        pending.push_back(other);
                    }
                }
                continue;
            }
            let operation = async {
                match work {
                    Work::Bind(response, reply) => {
                        let binding = output.0.registration.get().unwrap().binding;
                        let matches = binding.response_id == response.response_id
                            && Some(binding.context_id) == response.context_id
                            && response.key.request_id == output.0.request_id;
                        let _ = reply.send(if matches {
                            Ok(())
                        } else {
                            Err(anyhow::anyhow!("original output binding changed"))
                        });
                        anyhow::ensure!(matches, "original output binding changed");
                    }
                    Work::Action { hold_id, action } => {
                        sequence = sequence
                            .checked_add(1)
                            .context("control sequence exhausted")?;
                        anyhow::ensure!(
                            sequence <= MAX_CONTROL_COMMANDS as u64,
                            "control command capacity exhausted"
                        );
                        self.control_output(output, sequence, hold_id, action)
                            .await?;
                    }
                }
                Ok::<_, anyhow::Error>(())
            };
            self.output_operation(output, operation).await?;
        }
    }

    /// Keep the same in-flight operation when a semantic confirmation removes
    /// the temporary deadline. Hard hold/output deadlines never move forward.
    async fn output_operation<T>(
        &self,
        output: &VoiceOutput,
        operation: impl std::future::Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        tokio::pin!(operation);
        let mut changed = output.0.changed.subscribe();
        loop {
            let deadline = output.0.gate.lock().unwrap().deadline();
            anyhow::ensure!(output.is_alive(), "original output no longer active");
            tokio::select! { biased;
                _ = self.stop.cancelled() => anyhow::bail!("server is shutting down"),
                _ = output.0.cancel.cancelled() => anyhow::bail!("original output cancelled"),
                result = changed.changed() => { result.context("output gate owner ended")?; },
                _ = tokio::time::sleep_until(deadline) => anyhow::bail!("original output deadline"),
                result = &mut operation => return result,
            }
        }
    }

    async fn bind_output(
        &self,
        output: &VoiceOutput,
        response: &ResponseRecord,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            response.key.user_id == output.0.owner
                && response.key.request_id == output.0.request_id
                && response.channel_id == output.0.channel.channel_id(),
            "canonical Response differs from original producer"
        );
        let stored = bounded(
            self.repository
                .response(output.0.owner, response.response_id),
        )
        .await?;
        anyhow::ensure!(
            stored.intent_digest == response.intent_digest && !stored.cancel_requested,
            "canonical Response changed or cancelled"
        );
        let binding = OutputBinding {
            scope: output.0.scope,
            response_id: response.response_id,
            context_id: response
                .context_id
                .context("canonical Response has no Context")?,
            server_instance_id: self.instance_id,
        };
        let registration = output.0.channel.register_output(binding).await?;
        registration.validate()?;
        anyhow::ensure!(
            registration.binding == binding,
            "registered output binding differs"
        );
        // Record the original registration even if later durable admission fails,
        // so cleanup still fences the real peer. No Start is authorized yet.
        output
            .0
            .registration
            .set(registration.clone())
            .map_err(|_| anyhow::anyhow!("output already registered"))?;
        output
            .0
            .audio_budget
            .set(
                crate::providers::tts::audio_budget::ResponseAudioBudget::for_response(
                    response.response_id,
                )?,
            )
            .map_err(|_| anyhow::anyhow!("original audio budget already bound"))?;
        let durable = self
            .repository
            .register_control(output.0.owner, registration)
            .await?;
        anyhow::ensure!(
            durable.status == OutputControlStatus::Active,
            "original control owner retired"
        );
        output.tighten_deadline(durable.deadline_at);
        anyhow::ensure!(
            output.is_alive(),
            "original output expired during registration"
        );
        Ok(())
    }

    async fn control_output(
        &self,
        output: &VoiceOutput,
        sequence: u64,
        hold_id: Uuid,
        action: ControlAction,
    ) -> anyhow::Result<()> {
        let registration = output
            .0
            .registration
            .get()
            .context("output not registered")?;
        let command = BoundControlCommand {
            binding: registration.binding,
            expected_backend: registration.backend.clone(),
            command: ControlCommand {
                scope: output.0.scope,
                hold_id,
                command_id: Uuid::new_v4(),
                sequence,
                action,
            },
        };
        let admission = self
            .repository
            .admit_control(output.0.owner, self.instance_id, &command)
            .await;
        let received = match admission {
            Ok(ControlAdmission::Fresh(permit)) => {
                output.tighten_hold_deadline(hold_id, permit.deadline_at());
                anyhow::ensure!(
                    permit.command() == &command && permit.deadline_at() > chrono::Utc::now(),
                    "original control permit expired"
                );
                output.0.channel.control_output(permit.command()).await
            }
            // Existing or an unknown database commit never authorizes another
            // Control. Read only the original command; unavailable stays unknown.
            Ok(ControlAdmission::Existing(existing)) => {
                output.tighten_hold_deadline(hold_id, existing.deadline_at);
                output.0.channel.observe_output_control(&command).await
            }
            Err(
                crate::modules::delivery::repository::control::ControlLedgerError::CommitUnknown {
                    ..
                },
            ) => output.0.channel.observe_output_control(&command).await,
            Err(error) => return Err(error.into()),
        };
        let receipt = match received {
            Ok(receipt) => receipt,
            Err(error) => {
                let _ = self
                    .repository
                    .unknown_control(&command, &error.to_string())
                    .await;
                // A lost Control ACK is reconciled by Observe, never by replay.
                output.0.channel.observe_output_control(&command).await?
            }
        };
        self.apply_control_receipt(output, &command, receipt).await
    }

    async fn apply_control_receipt(
        &self,
        output: &VoiceOutput,
        command: &BoundControlCommand,
        receipt: BoundControlReceipt,
    ) -> anyhow::Result<()> {
        receipt.validate(command)?;
        let recorded = self.repository.record_control(&receipt).await?;
        // Store a late observation as a fact before checking permission to open
        // the local gate. Newly admitted audio may shorten the original scope.
        output.tighten_deadline(recorded.scope.deadline_at);
        output.tighten_hold_deadline(command.command.hold_id, recorded.command.deadline_at);
        anyhow::ensure!(
            recorded.scope.status == OutputControlStatus::Active
                && recorded.scope.deadline_at > chrono::Utc::now()
                && recorded.command.deadline_at > chrono::Utc::now()
                && output.is_alive(),
            "original output retired while control was in flight"
        );
        let current = recorded
            .scope
            .latest_receipt
            .as_ref()
            .context("control has no current device observation")?;
        anyhow::ensure!(
            current.receipt.snapshot.phase != ControlPhase::Retired,
            "device output retired"
        );
        if let ControlAction::Resume { authorization } = command.command.action {
            if receipt.receipt.outcome == CommandOutcome::Applied {
                anyhow::ensure!(
                    current.observation_sequence == receipt.observation_sequence
                        && current.receipt.snapshot.phase == ControlPhase::Running,
                    "resume is not the current device state"
                );
                let resumed = output.0.gate.lock().unwrap().resume(
                    command.command.hold_id,
                    authorization,
                    Instant::now(),
                );
                anyhow::ensure!(
                    resumed,
                    "new speech or local retirement superseded in-flight resume"
                );
                output.signal();
            }
            // A proved stale Resume rejection keeps the original gate held.
        } else {
            anyhow::ensure!(
                receipt.receipt.outcome == CommandOutcome::Applied
                    && matches!(
                        current.receipt.snapshot.phase,
                        ControlPhase::Pausing | ControlPhase::Held
                    ),
                "device did not accept the original hold"
            );
        }
        Ok(())
    }
}
