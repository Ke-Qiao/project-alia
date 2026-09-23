//! Instance-owned supervision. A dropped caller only loses its reply receiver;
//! it cannot drop the sole media/progress owner or authorize another dispatch.
use super::{repository::DeliveryRepository, types::*};
use crate::channels::adapter::{
    AudioDeliveryReport, AudioPayloadStream, ChannelAdapter, ChannelTarget, PlaybackObserver,
    TextObserver,
};
use alia_core::delivery::{ObservationEnd, PlaybackProgress};
use async_trait::async_trait;
use futures::FutureExt;
use serde_json::Value;
use std::{
    collections::HashMap,
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
mod media;
mod output;
pub use media::TextDelivery;
use media::{DeliveryInput, MediaIdentity, MediaProgress, MediaReport};
pub use output::{OutputProducer, VoiceOutput};

const DB_TIMEOUT: Duration = Duration::from_secs(2);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SUPERVISORS: usize = 64;

pub struct AudioDelivery {
    pub response_id: Uuid,
    pub output_key: String,
    pub content: Value,
    pub channel: Arc<dyn ChannelAdapter>,
    pub target: ChannelTarget,
    pub audio: AudioPayloadStream,
    pub cancel: CancellationToken,
}
struct Command {
    input: DeliveryInput,
    media_owner: Option<output::MediaOwner>,
    reply: oneshot::Sender<DeliveryResult<DeliveryRecord>>,
}

pub struct DeliveryService {
    repository: DeliveryRepository,
    instance_id: Uuid,
    stop: CancellationToken,
    closed: AtomicBool,
    admission_boundary: StdMutex<()>,
    running: AtomicBool,
    sender: mpsc::Sender<Command>,
    receiver: Mutex<Option<mpsc::Receiver<Command>>>,
    output_sender: mpsc::Sender<output::NewOutput>,
    output_receiver: Mutex<Option<mpsc::Receiver<output::NewOutput>>>,
    outputs: StdMutex<HashMap<alia_core::delivery::VoiceScope, VoiceOutput>>,
    task: Mutex<Option<JoinHandle<anyhow::Result<()>>>>,
    /// Only reservation + durable admission are serialized. Media I/O never holds
    /// this gate. A per-key gate can replace it if measured admission needs demand it.
    admission: Mutex<()>,
    active: StdMutex<HashMap<Uuid, (Uuid, CancellationToken)>>,
    failures: StdMutex<Vec<String>>,
}

impl DeliveryService {
    pub fn new(repository: DeliveryRepository, parent: &CancellationToken) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel(MAX_SUPERVISORS);
        let (output_sender, output_receiver) = mpsc::channel(MAX_SUPERVISORS);
        Arc::new(Self {
            repository,
            instance_id: Uuid::new_v4(),
            stop: parent.child_token(),
            closed: AtomicBool::new(false),
            admission_boundary: StdMutex::new(()),
            running: AtomicBool::new(false),
            sender,
            receiver: Mutex::new(Some(receiver)),
            output_sender,
            output_receiver: Mutex::new(Some(output_receiver)),
            outputs: StdMutex::new(HashMap::new()),
            task: Mutex::new(None),
            admission: Mutex::new(()),
            active: StdMutex::new(HashMap::new()),
            failures: StdMutex::new(Vec::new()),
        })
    }
    pub async fn start(self: &Arc<Self>) -> anyhow::Result<()> {
        let mut task = self.task.lock().await;
        anyhow::ensure!(
            !self.closed.load(Ordering::Acquire) && !self.stop.is_cancelled(),
            "delivery is closing"
        );
        if task.is_some() {
            return Ok(());
        }
        bounded(self.repository.recover_expired(256)).await?;
        self.repository.recover_expired_controls(256).await?;
        let rx = self
            .receiver
            .lock()
            .await
            .take()
            .ok_or_else(|| anyhow::anyhow!("delivery cannot restart"))?;
        let outputs = self
            .output_receiver
            .lock()
            .await
            .take()
            .ok_or_else(|| anyhow::anyhow!("output control cannot restart"))?;
        self.running.store(true, Ordering::Release);
        let service = self.clone();
        *task = Some(tokio::spawn(async move {
            let result = service.clone().run(rx, outputs).await;
            service.running.store(false, Ordering::Release);
            result
        }));
        Ok(())
    }
    pub fn close_admission(&self) {
        let _boundary = self.admission_boundary.lock().unwrap();
        self.closed.store(true, Ordering::Release);
    }
    pub fn shutdown_pending(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.close_admission();
        self.stop.cancel();
        for output in self.outputs.lock().unwrap().values() {
            output.retire();
        }
        for (_, cancel) in self.active.lock().unwrap().values() {
            cancel.cancel();
        }
        let deadline = tokio::time::Instant::now() + SHUTDOWN_TIMEOUT;
        let mut task = tokio::time::timeout_at(deadline, self.task.lock())
            .await
            .map_err(|_| {
                anyhow::anyhow!("delivery shutdown incomplete: another waiter still owns join")
            })?;
        if let Some(handle) = task.as_mut() {
            match tokio::time::timeout_at(deadline, handle).await {
                Ok(result) => {
                    task.take();
                    if let Err(error) = result
                        .map_err(anyhow::Error::from)
                        .and_then(|result| result)
                    {
                        self.failures
                            .lock()
                            .unwrap()
                            .push(format!("delivery supervisor: {error:#}"));
                    }
                }
                Err(_) => anyhow::bail!(
                    "delivery shutdown incomplete: supervised output still owns channels and ledger"
                ),
            }
        }
        let failures = self.failures.lock().unwrap();
        anyhow::ensure!(
            failures.is_empty(),
            "delivery shutdown failed: {}",
            failures.join("; ")
        );
        Ok(())
    }
    pub async fn prepare(&self, input: NewResponse) -> DeliveryResult<ResponseRecord> {
        self.ensure_open()?;
        bounded(self.repository.prepare(input)).await
    }
    pub async fn get_response(&self, owner: Uuid, id: Uuid) -> DeliveryResult<ResponseRecord> {
        bounded(self.repository.response(owner, id)).await
    }
    pub async fn get_delivery(&self, owner: Uuid, id: Uuid) -> DeliveryResult<DeliveryRecord> {
        bounded(self.repository.get(owner, id)).await
    }
    pub async fn list_deliveries(
        &self,
        owner: Uuid,
        response: Uuid,
    ) -> DeliveryResult<Vec<DeliveryRecord>> {
        bounded(self.repository.list(owner, response)).await
    }
    pub async fn response_events(
        &self,
        owner: Uuid,
        response: Uuid,
        after: i64,
        limit: u32,
    ) -> DeliveryResult<Vec<DeliveryEvent>> {
        if after < 0 || !(1..=256).contains(&limit) {
            return Err(DeliveryError::InvalidInput(
                "event cursor must be nonnegative and limit must be between 1 and 256".into(),
            ));
        }
        bounded(self.repository.events(owner, response, after, limit)).await
    }
    pub async fn cancel_response(
        &self,
        owner: Uuid,
        response: Uuid,
    ) -> DeliveryResult<ResponseRecord> {
        // Signal the original media owners even if the cancellation write is slow.
        // The owner check precedes this signal; knowing another response ID is insufficient.
        bounded(self.repository.response(owner, response)).await?;
        for output in self.outputs.lock().unwrap().values() {
            if output.response_id() == Some(response) {
                output.retire();
            }
        }
        for (id, cancel) in self.active.lock().unwrap().values() {
            if *id == response {
                cancel.cancel();
            }
        }
        bounded(
            self.repository
                .request_cancel(owner, response, "response cancellation requested"),
        )
        .await?;
        bounded(self.repository.response(owner, response)).await
    }
    pub async fn deliver_audio(&self, input: AudioDelivery) -> DeliveryResult<DeliveryRecord> {
        self.deliver(input.into()).await
    }
    pub async fn deliver_text(&self, input: TextDelivery) -> DeliveryResult<DeliveryRecord> {
        self.deliver(input.into()).await
    }
    pub async fn text_chunks(
        &self,
        owner: Uuid,
        id: Uuid,
        after: Option<u64>,
        limit: u32,
    ) -> DeliveryResult<Vec<alia_core::text_delivery::TextChunk>> {
        if !(1..=256).contains(&limit) {
            return Err(DeliveryError::InvalidInput(
                "text chunk page limit must be between 1 and 256".into(),
            ));
        }
        bounded(self.repository.text_chunks(owner, id, after, limit)).await
    }
    async fn deliver(&self, input: DeliveryInput) -> DeliveryResult<DeliveryRecord> {
        let receive = {
            let _boundary = self.admission_boundary.lock().unwrap();
            self.ensure_open()?;
            if !self.running.load(Ordering::Acquire) {
                return Err(DeliveryError::InvalidTransition(
                    "delivery supervision has not started".into(),
                ));
            }
            // Queue ownership starts before the caller can drop its producer.
            // A missing/retired output may still be an idempotent fact lookup;
            // supervise checks that fact before requiring a live media owner.
            let media_owner = if input.payload.medium() == DeliveryMedium::Audio {
                input.target.voice.and_then(|scope| {
                    self.outputs
                        .lock()
                        .unwrap()
                        .get(&scope)
                        .and_then(|output| output.retain_media(input.response_id).ok())
                })
            } else {
                None
            };
            let (reply, receive) = oneshot::channel();
            self.sender
                .try_send(Command {
                    input,
                    media_owner,
                    reply,
                })
                .map_err(|_| DeliveryError::CapacityExceeded)?;
            receive
        };
        receive.await.map_err(|_| {
            DeliveryError::InvalidTransition("delivery supervisor stopped without result".into())
        })?
    }
    fn ensure_open(&self) -> DeliveryResult<()> {
        if self.closed.load(Ordering::Acquire) || self.stop.is_cancelled() {
            Err(DeliveryError::InvalidTransition(
                "delivery admission is closed".into(),
            ))
        } else {
            Ok(())
        }
    }
    async fn run(
        self: Arc<Self>,
        mut receiver: mpsc::Receiver<Command>,
        mut outputs: mpsc::Receiver<output::NewOutput>,
    ) -> anyhow::Result<()> {
        let mut tasks = JoinSet::new();
        let mut output_tasks = JoinSet::new();
        let mut recovery = tokio::time::interval(Duration::from_secs(5));
        recovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {biased;
             _=self.stop.cancelled()=>break,
             result=tasks.join_next(),if !tasks.is_empty()=>{if let Some(Err(error))=result {self.failures.lock().unwrap().push(format!("delivery task panicked: {error}"));}},
             result=output_tasks.join_next(),if !output_tasks.is_empty()=>{if let Some(Err(error))=result {self.failures.lock().unwrap().push(format!("output control task panicked: {error}"));self.stop.cancel();}},
             next=outputs.recv(),if output_tasks.len()<MAX_SUPERVISORS=>match next{Some(input)=>{let service=self.clone();output_tasks.spawn(async move {service.supervise_output(input).await;});},None=>break},
             command=receiver.recv(),if tasks.len()<MAX_SUPERVISORS=>match command{Some(command)=>{let service=self.clone();tasks.spawn(async move {let result=service.supervise(command.input, command.media_owner).await;let _=command.reply.send(result);});},None=>break},
             _=recovery.tick()=>{if let Err(error)=bounded(self.repository.recover_expired(256)).await{self.failures.lock().unwrap().push(format!("delivery recovery: {error}"));self.stop.cancel();} if let Err(error)=self.repository.recover_expired_controls(256).await{self.failures.lock().unwrap().push(format!("output control recovery: {error}"));self.stop.cancel();}},
            }
        }
        self.close_admission();
        self.stop.cancel();
        receiver.close();
        outputs.close();
        for output in self.outputs.lock().unwrap().values() {
            output.retire();
        }
        while let Some(input) = outputs.recv().await {
            self.clone().supervise_output(input).await;
        }
        while let Some(command) = receiver.recv().await {
            let _ = command.reply.send(Err(DeliveryError::InvalidTransition(
                "delivery stopped before admission".into(),
            )));
        }
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                self.failures
                    .lock()
                    .unwrap()
                    .push(format!("delivery join failed: {error}"));
            }
        }
        while let Some(result) = output_tasks.join_next().await {
            if let Err(error) = result {
                self.failures
                    .lock()
                    .unwrap()
                    .push(format!("output control join failed: {error}"));
            }
        }
        Ok(())
    }
    async fn existing_delivery(
        &self,
        owner: Uuid,
        input: &mut DeliveryInput,
    ) -> DeliveryResult<Option<DeliveryRecord>> {
        let response = bounded(self.repository.response(owner, input.response_id)).await?;
        if input.channel.owner_user_id() != Some(owner)
            || input.channel.channel_id() != response.channel_id
        {
            return Err(DeliveryError::Unauthorized(
                "pinned channel does not own response target".into(),
            ));
        }
        if let Some(existing) = bounded(self.repository.find(
            owner,
            input.response_id,
            &input.output_key,
        ))
        .await?
        {
            if existing.content != input.content
                || existing.media_kind != input.payload.medium()
                || (existing.media_kind == DeliveryMedium::Audio
                    && existing.identity.map(|id| id.scope) != input.target.voice)
            {
                return Err(DeliveryError::IdempotencyConflict);
            }
            // A retry sees the durable fact, never obtains another dispatch owner.
            return Ok(Some(existing));
        }
        Ok(None)
    }
    async fn supervise(
        self: &Arc<Self>,
        mut input: DeliveryInput,
        media_owner: Option<output::MediaOwner>,
    ) -> DeliveryResult<DeliveryRecord> {
        let owner = Uuid::parse_str(&input.target.user_id)
            .map_err(|_| DeliveryError::Unauthorized("target has no linked account".into()))?;
        if let Some(existing) = self.existing_delivery(owner, &mut input).await? {
            return Ok(existing);
        }
        // Keep the queue's original owner through native tail completion. Never
        // hold the admission mutex while waiting for a reversible hold.
        let _media_owner = media_owner;
        if input.payload.medium() == DeliveryMedium::Audio && input.target.voice.is_some() {
            let output = _media_owner
                .as_ref()
                .ok_or_else(|| {
                    DeliveryError::InvalidTransition(
                        "voice output has no original producer owner".into(),
                    )
                })?
                .output();
            output
                .wait_open()
                .await
                .map_err(|e| DeliveryError::InvalidTransition(e.to_string()))?;
            input.payload = match input.payload {
                media::Payload::Audio(stream) => media::Payload::Audio(output.gate_stream(stream)),
                other => other,
            };
        }
        let gate = tokio::select! {biased;_=self.stop.cancelled()=>return Err(DeliveryError::InvalidTransition("delivery stopped before reservation".into())),guard=self.admission.lock()=>guard};
        if let Some(existing) = self.existing_delivery(owner, &mut input).await? {
            return Ok(existing);
        }
        self.ensure_open()?;
        let media_id = Uuid::new_v4();
        let identity = match input.payload.medium() {
            DeliveryMedium::Audio => {
                let identity = input
                    .channel
                    .reserve_playback(&input.target, media_id)
                    .map_err(|error| {
                        DeliveryError::InvalidInput(format!("reserve playback: {error:#}"))
                    })?;
                if identity.is_some_and(|id| {
                    id.playback_id != media_id || Some(id.scope) != input.target.voice
                }) || (input.target.voice.is_some() && identity.is_none())
                {
                    return Err(DeliveryError::InvalidInput(
                        "reserved playback identity differs from original target".into(),
                    ));
                }
                MediaIdentity::Audio(identity)
            }
            DeliveryMedium::Text => {
                let identity = input
                    .channel
                    .reserve_text(&input.target, media_id)
                    .map_err(|error| {
                        DeliveryError::InvalidInput(format!("reserve text: {error:#}"))
                    })?;
                if identity.is_some_and(|id| id.text_id != media_id || !id.is_valid()) {
                    return Err(DeliveryError::InvalidInput(
                        "reserved text identity differs from server allocation".into(),
                    ));
                }
                MediaIdentity::Text(identity)
            }
        };
        let record = match identity {
            MediaIdentity::Audio(identity) => {
                bounded(self.repository.prepare_delivery(
                    owner,
                    NewDelivery {
                        response_id: input.response_id,
                        output_key: input.output_key,
                        content: input.content,
                        identity,
                        owner_id: self.instance_id,
                    },
                ))
                .await?
            }
            MediaIdentity::Text(identity) => {
                bounded(self.repository.prepare_text_delivery(
                    owner,
                    NewTextDelivery {
                        response_id: input.response_id,
                        output_key: input.output_key,
                        content: input.content,
                        identity,
                        owner_id: self.instance_id,
                    },
                ))
                .await?
            }
        };
        let cancel = input.cancel.child_token();
        self.active
            .lock()
            .unwrap()
            .insert(record.delivery_id, (input.response_id, cancel.clone()));
        let _active = ActiveGuard {
            service: self.clone(),
            id: record.delivery_id,
        };
        drop(gate);
        if cancel.is_cancelled() || self.stop.is_cancelled() || self.closed.load(Ordering::Acquire)
        {
            bounded(self.repository.request_cancel(
                owner,
                input.response_id,
                "cancelled before dispatch",
            ))
            .await?;
            return bounded(self.repository.get(owner, record.delivery_id)).await;
        }
        let Some(permit) = bounded(self.repository.dispatch(
            owner,
            record.delivery_id,
            self.instance_id,
        ))
        .await?
        else {
            return bounded(self.repository.get(owner, record.delivery_id)).await;
        };
        let observer = Arc::new(DurableObserver {
            repository: self.repository.clone(),
            permit: Arc::new(permit),
        });
        let adapter_cancel = CancellationToken::new();
        let mut delivery = Box::pin(input.payload.deliver(
            input.channel.as_ref(),
            &input.target,
            identity,
            observer.clone(),
            adapter_cancel.clone(),
        ));
        // Poll lease work concurrently with the adapter, never await it inside
        // a selected branch while the paused adapter may hold an observer DB lock.
        let heartbeat = async {
            let mut tick = tokio::time::interval_at(
                tokio::time::Instant::now() + Duration::from_secs(5),
                Duration::from_secs(5),
            );
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                if let Err(error) = bounded(self.repository.heartbeat(&observer.permit)).await {
                    return error;
                }
            }
        };
        let mut heartbeat = Some(Box::pin(heartbeat));
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(OBSERVATION_SECONDS as u64);
        let mut adapter_returned = false;
        let mut terminal_error = None;
        let reason = loop {
            tokio::select! {biased;
             _=self.stop.cancelled()=>break "server shutdown".to_string(),
             _=cancel.cancelled()=>break "response cancelled".to_string(),
             _=tokio::time::sleep_until(deadline)=>break "absolute observation deadline".to_string(),
             result=AssertUnwindSafe(&mut delivery).catch_unwind()=>{heartbeat.take();match result{
              Ok(Ok(report))=>{
               adapter_returned=true;
               if report.progress.is_none() {
                break "observation ended without a final checkpoint".to_string();
               }
               if matches!(report.observation_end, ObservationEnd::Unknown { .. })
                && report.progress.as_ref().is_some_and(|progress| progress.end().is_none()) {
                // A transport timeout is local information, not a device
                // terminal receipt. Preserve its last real observation and
                // try the independent precise checkpoint before freezing Unknown.
                if let Some(progress) = report.progress {
                 if let Err(error) = bounded(observer.record(progress,false)).await {
                  self.failures.lock().unwrap().push(format!("last delivery observation: {error}"));
                 }
                }
                break "transport observation ended before a device terminal checkpoint".to_string();
               }
               match self.finish_report(&observer,report).await {
                Ok(record) if record.status!=DeliveryStatus::Unknown=>return Ok(record),
                Ok(_)=>break "observation ended without trustworthy completion".to_string(),
                Err(error)=>{self.failures.lock().unwrap().push(format!("delivery terminal persistence: {error}"));let reason=format!("terminal evidence unavailable: {error}");terminal_error=Some(error);break reason;}
               }
              },
              Ok(Err(error))=>{adapter_returned=true;break format!("delivery transport failed: {error:#}");},
              Err(_)=>{
               adapter_cancel.cancel();self.failures.lock().unwrap().push("delivery adapter panicked; original future quarantined with dependencies".into());
               // Retain the future and pinned channel on panic. Their cleanup semantics
               // are unknown; shutdown must remain incomplete, not tear down dependencies.
               self.persist_unknown(&observer,"delivery adapter panicked").await;
               std::future::pending::<()>().await;unreachable!();
              }
             }},
             error=async {heartbeat.as_mut().unwrap().await}=>break format!("delivery lease unavailable: {error}"),
            }
        };
        drop(heartbeat); // Release a paused lease transaction before cleanup DB work.
        adapter_cancel.cancel();
        // The adapter gets a bounded opportunity to close its observation stream.
        // A separate exact fence is necessary if cancellation interrupts its ACK path.
        let mut fence_future = Box::pin(identity.cancel(input.channel.as_ref(), &reason));
        let (fence_progress, fence_pending) = match tokio::time::timeout(
            CANCEL_TIMEOUT,
            AssertUnwindSafe(&mut fence_future).catch_unwind(),
        )
        .await
        {
            Ok(Ok(result)) => (result.ok(), false),
            Ok(Err(_)) => {
                self.failures.lock().unwrap().push("playback cancellation adapter panicked; fence and original delivery futures quarantined".into());
                self.persist_unknown(&observer, "playback cancellation adapter panicked")
                    .await;
                std::future::pending::<()>().await;
                unreachable!();
            }
            Err(_) => (None, true),
        };
        let mut observed_final = match fence_progress {
            Some(progress) => self.save_checkpoint(&observer, progress).await,
            None => None,
        };
        let mut still_running = false;
        if !adapter_returned {
            match tokio::time::timeout(
                CANCEL_TIMEOUT,
                AssertUnwindSafe(&mut delivery).catch_unwind(),
            )
            .await
            {
                Ok(Ok(Ok(report))) if report.progress.is_some() => {
                    if let Ok(record) = self.finish_report(&observer, report).await {
                        observed_final = Some(record);
                    }
                }
                Ok(Err(_)) => {
                    self.failures.lock().unwrap().push(
                        "delivery adapter panicked during cancellation; future quarantined".into(),
                    );
                    self.persist_unknown(
                        &observer,
                        "delivery adapter panicked during cancellation",
                    )
                    .await;
                    std::future::pending::<()>().await;
                }
                Err(_) => still_running = true,
                _ => {}
            }
        }
        let result = if let Some(record) = observed_final {
            Ok(record)
        } else {
            bounded(self.repository.finish_unknown(&observer.permit, &reason)).await
        };
        if let Err(error) = &result {
            self.failures
                .lock()
                .unwrap()
                .push(format!("delivery terminal persistence: {error}"));
        }
        if still_running || fence_pending {
            // A deadline is a reporting budget, not proof that the adapter has
            // stopped. Retain its original future and channel until actual exit.
            // Unknown remains frozen; a late final checkpoint is retained as an
            // audit fact, never permission to resume output or replay the input.
            self.failures.lock().unwrap().push(
                "delivery cancellation deadline exceeded; retaining adapter cleanup ownership"
                    .into(),
            );
            let (late_delivery, late_fence) = futures::join!(
                async {
                    if still_running {
                        Some(AssertUnwindSafe(&mut delivery).catch_unwind().await)
                    } else {
                        None
                    }
                },
                async {
                    if fence_pending {
                        Some(AssertUnwindSafe(&mut fence_future).catch_unwind().await)
                    } else {
                        None
                    }
                },
            );
            if matches!(late_delivery, Some(Err(_))) || matches!(late_fence, Some(Err(_))) {
                self.persist_unknown(&observer, "delivery cleanup adapter panicked")
                    .await;
                std::future::pending::<()>().await;
            }
            if let Some(Ok(Ok(report))) = late_delivery {
                if let Err(error) = self.finish_report(&observer, report).await {
                    self.failures
                        .lock()
                        .unwrap()
                        .push(format!("late delivery checkpoint: {error}"));
                }
            }
            if let Some(Ok(Ok(progress))) = late_fence {
                self.save_checkpoint(&observer, progress).await;
            }
        }
        if let Some(error) = terminal_error {
            return Err(error);
        }
        result
    }
    async fn persist_unknown(&self, observer: &DurableObserver, reason: &str) {
        if let Err(error) = bounded(self.repository.finish_unknown(&observer.permit, reason)).await
        {
            self.failures
                .lock()
                .unwrap()
                .push(format!("delivery Unknown persistence: {error}"));
        }
    }
    async fn save_checkpoint(
        &self,
        observer: &DurableObserver,
        progress: MediaProgress,
    ) -> Option<DeliveryRecord> {
        match bounded(observer.record(progress, true)).await {
            Ok(record) => Some(record),
            Err(error) => {
                self.failures
                    .lock()
                    .unwrap()
                    .push(format!("delivery cancellation checkpoint: {error}"));
                None
            }
        }
    }
    async fn finish_report(
        &self,
        observer: &DurableObserver,
        report: MediaReport,
    ) -> DeliveryResult<DeliveryRecord> {
        match report.progress {
            Some(progress) if progress.end().as_ref() == Some(&report.observation_end) => {
                bounded(observer.record(progress, true)).await
            }
            Some(_) => Err(DeliveryError::InvalidEvidence(
                "terminal report contradicts progress evidence".into(),
            )),
            None => {
                let reason = match report.observation_end {
                    ObservationEnd::Unknown { reason } => reason,
                    _ => "transport returned a terminal claim without progress evidence".into(),
                };
                bounded(self.repository.finish_unknown(&observer.permit, &reason)).await
            }
        }
    }
}
struct ActiveGuard {
    service: Arc<DeliveryService>,
    id: Uuid,
}
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.service.active.lock().unwrap().remove(&self.id);
    }
}
struct DurableObserver {
    repository: DeliveryRepository,
    permit: Arc<DispatchPermit>,
}
impl DurableObserver {
    async fn record(
        &self,
        progress: MediaProgress,
        checkpoint: bool,
    ) -> DeliveryResult<DeliveryRecord> {
        match progress {
            MediaProgress::Audio(progress) if checkpoint => {
                self.repository
                    .terminal_checkpoint(&self.permit, progress)
                    .await
            }
            MediaProgress::Audio(progress) => {
                self.repository.progress(&self.permit, progress).await
            }
            MediaProgress::Text(progress) => {
                self.repository
                    .text_progress(&self.permit, progress, checkpoint)
                    .await
            }
        }
    }
}
#[async_trait]
impl TextObserver for DurableObserver {
    async fn start(&self) -> anyhow::Result<()> {
        Ok(bounded(self.repository.text_start(&self.permit)).await?)
    }
    async fn offered(&self, chunk: &alia_core::text_delivery::TextChunk) -> anyhow::Result<()> {
        Ok(bounded(self.repository.offer_text_chunk(&self.permit, chunk)).await?)
    }
    async fn end(&self, chunks: u64, utf8_bytes: u64) -> anyhow::Result<()> {
        Ok(bounded(
            self.repository
                .offer_text_end(&self.permit, chunks, utf8_bytes),
        )
        .await?)
    }
    async fn progress(
        &self,
        progress: alia_core::text_delivery::TextProgress,
    ) -> anyhow::Result<()> {
        bounded(self.repository.text_progress(&self.permit, progress, false)).await?;
        Ok(())
    }
}
#[async_trait]
impl PlaybackObserver for DurableObserver {
    async fn start(&self, rate: u32, channels: u16) -> anyhow::Result<()> {
        Ok(bounded(self.repository.start(&self.permit, rate, channels)).await?)
    }
    async fn offered(&self, frames: u64) -> anyhow::Result<()> {
        Ok(bounded(self.repository.offered(&self.permit, frames)).await?)
    }
    async fn progress(&self, progress: PlaybackProgress) -> anyhow::Result<()> {
        bounded(self.repository.progress(&self.permit, progress)).await?;
        Ok(())
    }
}
async fn bounded<T>(
    future: impl std::future::Future<Output = DeliveryResult<T>>,
) -> DeliveryResult<T> {
    tokio::time::timeout(DB_TIMEOUT, future)
        .await
        .map_err(|_| {
            DeliveryError::Database(sea_orm::DbErr::Custom(
                "delivery database deadline exceeded; commit outcome may be unknown".into(),
            ))
        })?
}
