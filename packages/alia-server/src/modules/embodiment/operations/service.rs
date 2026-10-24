use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use futures::FutureExt;
use sea_orm::DatabaseConnection;
use serde_json::Value;
use tokio::sync::{Semaphore, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    AdapterDescriptor, OperationFeedback, OperationRecord, OperationRequest, PhysicalAdapter,
    repository::{CallLease, Repository},
    tools,
};
use crate::{
    modules::platform::lifecycle::RuntimeTasks,
    providers::adapter::ToolContext,
    tools::entry::{CompiledSchema, ToolEntry, compile_schema},
};

const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ADAPTERS: usize = 32;

pub(super) struct Registered {
    pub descriptor: AdapterDescriptor,
    pub adapter: Arc<dyn PhysicalAdapter>,
    schema: CompiledSchema,
}

#[derive(Default)]
struct Catalog {
    adapters: HashMap<String, Arc<Registered>>,
    entries: Vec<Arc<ToolEntry>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Call {
    Start,
    Observe,
    Cancel,
}

pub struct EmbodimentService {
    repository: Repository,
    catalog: Mutex<Catalog>,
    started: AtomicBool,
    closed: AtomicBool,
    stop: CancellationToken,
    tasks: RuntimeTasks,
    slots: Arc<Semaphore>,
    cancel_slots: Arc<Semaphore>,
}

impl EmbodimentService {
    pub fn new(db: DatabaseConnection) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            repository: Repository(db),
            catalog: Mutex::new(Catalog {
                entries: vec![
                    tools::status(weak.clone()).expect("constant operation status tool schema"),
                ],
                ..Catalog::default()
            }),
            started: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            stop: CancellationToken::new(),
            tasks: RuntimeTasks::default(),
            slots: Arc::new(Semaphore::new(8)),
            cancel_slots: Arc::new(Semaphore::new(2)),
        })
    }

    /// Bootstrap-only: replacing a running adapter would silently change the
    /// implementation to which already approved work was bound.
    pub fn register_adapter(self: &Arc<Self>, adapter: Arc<dyn PhysicalAdapter>) -> Result<()> {
        let mut catalog = self.catalog.lock().expect("operation catalog");
        ensure!(
            !self.started.load(Ordering::Acquire) && !self.is_closed(),
            "operation adapter registration is closed"
        );
        let descriptor = adapter.descriptor();
        ensure!(
            !descriptor.owner_id.is_nil()
                && !descriptor.device_id.is_nil()
                && !descriptor.boot_id.is_nil(),
            "device grant and incarnation IDs are required"
        );
        ensure!(
            !descriptor.adapter_id.is_empty()
                && descriptor.adapter_id.len() <= 32
                && descriptor
                    .adapter_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "invalid adapter ID"
        );
        for value in [
            &descriptor.revision,
            &descriptor.capability,
            &descriptor.capability_pin,
        ] {
            ensure!(
                !value.trim().is_empty() && value.len() <= 256,
                "adapter implementation and capability pins are required and bounded"
            );
        }
        ensure!(
            !descriptor.resources.is_empty() && descriptor.resources.len() <= 16,
            "adapter requires 1..16 fixed exclusive resources"
        );
        let mut resources = HashSet::new();
        for resource in &descriptor.resources {
            ensure!(
                !resource.is_empty()
                    && resource.len() <= 128
                    && resource
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-.:/".contains(&b))
                    && resources.insert(resource),
                "invalid or duplicate device resource"
            );
        }
        ensure!(
            catalog.adapters.len() < MAX_ADAPTERS
                && !catalog.adapters.contains_key(&descriptor.adapter_id),
            "adapter capacity or duplicate ID"
        );
        let schema = compile_schema(&descriptor.goal_schema)?;
        let entries = tools::adapter_entries(Arc::downgrade(self), &descriptor)?;
        let id = descriptor.adapter_id.clone();
        catalog.adapters.insert(
            id,
            Arc::new(Registered {
                descriptor,
                adapter,
                schema,
            }),
        );
        catalog.entries.extend(entries);
        Ok(())
    }

    pub fn entries(&self) -> Vec<Arc<ToolEntry>> {
        self.catalog
            .lock()
            .expect("operation catalog")
            .entries
            .clone()
    }

    pub(super) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn adapter(&self, id: &str) -> Result<Arc<Registered>> {
        ensure!(!self.is_closed(), "operation service is stopping");
        self.catalog
            .lock()
            .expect("operation catalog")
            .adapters
            .get(id)
            .cloned()
            .context("physical adapter is unavailable")
    }

    pub async fn get(&self, owner: Uuid, id: Uuid) -> Result<Option<OperationRecord>> {
        self.repository.get(owner, id).await
    }
    pub async fn list(&self, owner: Uuid, limit: u64) -> Result<Vec<OperationRecord>> {
        self.repository.list(owner, limit).await
    }

    /// Only a matching Dispatched effect can reach the durable admission below.
    /// The worker owns both admission and its eventual outbound call, even when
    /// an HTTP/Execution caller drops its response receiver.
    pub async fn submit(
        self: &Arc<Self>,
        adapter_id: &str,
        context: &ToolContext,
        arguments: Value,
    ) -> Result<OperationRecord> {
        let registered = self.adapter(adapter_id)?;
        registered.schema.validate(&arguments)?;
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .context("physical operation call capacity exhausted")?;
        let owner = self.clone();
        let context = context.clone();
        let (send, receive) = oneshot::channel();
        self.tasks.try_spawn("embodiment submit", async move {
            let _permit = permit;
            match owner
                .repository
                .create(&registered.descriptor, &context, &arguments)
                .await
            {
                Ok((record, lease)) => {
                    let _ = send.send(Ok(record.clone()));
                    if let Some(lease) = lease {
                        owner
                            .call(registered, record, Call::Start, Some(lease))
                            .await?;
                    }
                }
                Err(error) => {
                    let _ = send.send(Err(error));
                }
            }
            Ok(())
        })?;
        receive.await.context("operation admission owner failed")?
    }

    pub async fn cancel(
        self: &Arc<Self>,
        adapter_id: &str,
        context: &ToolContext,
        id: Uuid,
    ) -> Result<OperationRecord> {
        let registered = self.adapter(adapter_id)?;
        let permit = self
            .cancel_slots
            .clone()
            .try_acquire_owned()
            .context("physical operation call capacity exhausted")?;
        let owner = self.clone();
        let context = context.clone();
        let (send, receive) = oneshot::channel();
        self.tasks.try_spawn("embodiment cancel", async move {
            let _permit = permit;
            let args = serde_json::json!({"operation_id":id});
            match owner
                .repository
                .cancel(&registered.descriptor, &context, &args, id)
                .await
            {
                Ok((record, dispatch)) => {
                    let _ = send.send(Ok(record.clone()));
                    if dispatch {
                        owner.call(registered, record, Call::Cancel, None).await?;
                    }
                }
                Err(error) => {
                    let _ = send.send(Err(error));
                }
            }
            Ok(())
        })?;
        receive
            .await
            .context("operation cancellation owner failed")?
    }

    async fn call(
        &self,
        registered: Arc<Registered>,
        record: OperationRecord,
        kind: Call,
        lease: Option<CallLease>,
    ) -> Result<()> {
        let id = record.identity.operation_id;
        let lease = if let Some(lease) = lease {
            Some(lease)
        } else {
            let acquire = self.repository.lease(id, kind != Call::Observe);
            tokio::pin!(acquire);
            let result = tokio::select! {
                result=&mut acquire=>result,
                _=tokio::time::sleep(CALL_TIMEOUT)=>{
                    if let Err(error)=self.repository.unknown(id,"waiting for original physical call owner; cancellation remains queued").await {tracing::error!(%id,%error,"record physical operation wait");}
                    acquire.await
                }
            };
            match result {
                Ok(lease) => lease,
                Err(error) => {
                    return self
                        .repository
                        .unknown(id, &format!("physical call ordering unavailable: {error}"))
                        .await;
                }
            }
        };
        let Some(_lease) = lease else {
            return Ok(());
        };
        let record = self.repository.current(id).await?;
        if kind != Call::Start && record.status.is_terminal() && record.stopped.is_some() {
            return Ok(());
        }

        // Registration metadata is captured once. A mutable descriptor cannot
        // move an operation to a new boot/implementation behind the catalog.
        if !registered.descriptor.matches(&record.identity)
            || registered.adapter.descriptor().identity(id) != record.identity
        {
            return self
                .repository
                .unknown(id, "physical adapter binding changed")
                .await;
        }
        let expected_identity = record.identity.clone();
        let future = async {
            match kind {
                Call::Start => {
                    registered
                        .adapter
                        .start(OperationRequest {
                            identity: record.identity,
                            goal: record.goal,
                        })
                        .await
                }
                Call::Observe => registered.adapter.observe(record.identity).await,
                Call::Cancel => registered.adapter.cancel(record.identity).await,
            }
        };
        let future = std::panic::AssertUnwindSafe(future).catch_unwind();
        tokio::pin!(future);
        let result = tokio::select! {
            result=&mut future=>result,
            _=tokio::time::sleep(CALL_TIMEOUT)=>{
                // No abort: the original call and adapter remain inside the
                // tracked worker until their real completion.
                if let Err(error)=self.repository.unknown(id,"adapter call timed out; original call remains owned").await {tracing::error!(%id,%error,"record physical operation timeout");}
                future.await
            }
        };
        let feedback: Result<OperationFeedback> =
            result.unwrap_or_else(|_| Err(anyhow::anyhow!("physical adapter call panicked")));
        match feedback {
            Ok(feedback) => {
                if feedback.identity != expected_identity {
                    self.repository
                        .unknown(
                            id,
                            "adapter feedback belongs to a different operation or incarnation",
                        )
                        .await?;
                } else if let Err(error) = self.repository.feedback(feedback).await {
                    if error.is::<super::repository::SupersededFeedback>() {
                        tracing::debug!(%id,"discarded superseded operation observation");
                    } else {
                        self.repository
                            .unknown(id, &format!("adapter feedback rejected: {error}"))
                            .await?;
                    }
                }
            }
            Err(error) => {
                self.repository
                    .unknown(id, &format!("adapter call failed: {error}"))
                    .await?
            }
        }
        Ok(())
    }

    /// Bounded, query-only recovery. Existing durable IDs are never submitted
    /// again, including a crash between reserving resources and calling start.
    pub async fn reconcile_once(self: &Arc<Self>) -> Result<()> {
        ensure!(!self.is_closed(), "operation service is stopping");
        for record in self.repository.due().await? {
            let id = record.identity.operation_id;
            let registered = self
                .catalog
                .lock()
                .expect("operation catalog")
                .adapters
                .get(&record.identity.adapter_id)
                .cloned();
            let Some(registered) = registered.filter(|r| r.descriptor.matches(&record.identity))
            else {
                self.repository
                    .unknown(
                        id,
                        "matching device boot and adapter implementation are unavailable",
                    )
                    .await?;
                continue;
            };
            let Ok(permit) = self.slots.clone().try_acquire_owned() else {
                break;
            };
            let owner = self.clone();
            self.tasks.try_spawn("embodiment observation", async move {
                let _permit = permit;
                owner.call(registered, record, Call::Observe, None).await
            })?;
        }
        Ok(())
    }

    pub async fn start(self: &Arc<Self>) -> Result<()> {
        let _catalog = self.catalog.lock().expect("operation catalog");
        ensure!(!self.is_closed(), "operation service is stopping");
        if self.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let owner = self.clone();
        self.tasks.try_spawn("embodiment reconcile", async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {biased; _=owner.stop.cancelled()=>break, _=tick.tick()=>{}}
                if let Err(error) = owner.reconcile_once().await {
                    tracing::warn!(%error,"physical operation reconciliation failed");
                }
            }
            Ok(())
        })
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.closed.store(true, Ordering::Release);
        self.stop.cancel();
        self.tasks.close();
        self.tasks.wait().await
    }
}
