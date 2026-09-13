mod delivery_support;
use alia_core::{delivery::ObservationEnd, text_delivery::*};
use alia_server::{
    channels::adapter::*,
    module::ModuleLifecycle,
    modules::delivery::{DeliveryService, TextDelivery, types::*},
};
use async_trait::async_trait;
use delivery_support::*;
use futures::StreamExt;
use serde_json::json;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct Channel {
    owner: Uuid,
    instance: Uuid,
    calls: AtomicUsize,
    native_bytes: Arc<AtomicUsize>,
    progress: Arc<Mutex<Option<TextProgress>>>,
    cancel: Arc<AtomicBool>,
    active_write: Arc<AtomicBool>,
    changed: Arc<Notify>,
    entered: Arc<Semaphore>,
    blocked: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    legacy: bool,
}
impl Channel {
    fn new(owner: Uuid, blocked: Option<std::sync::mpsc::Receiver<()>>, legacy: bool) -> Arc<Self> {
        Arc::new(Self {
            owner,
            instance: Uuid::new_v4(),
            calls: Default::default(),
            native_bytes: Default::default(),
            progress: Default::default(),
            cancel: Default::default(),
            active_write: Default::default(),
            changed: Default::default(),
            entered: Arc::new(Semaphore::new(0)),
            blocked: Mutex::new(blocked),
            legacy,
        })
    }
}
impl ModuleLifecycle for Channel {
    fn name(&self) -> &'static str {
        "owned-text-test"
    }
}
#[async_trait]
impl ChannelAdapter for Channel {
    fn channel_type(&self) -> &'static str {
        "test"
    }
    fn channel_id(&self) -> &str {
        "test-device"
    }
    fn owner_user_id(&self) -> Option<Uuid> {
        Some(self.owner)
    }
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            text: true,
            ..Default::default()
        }
    }
    async fn send_message(&self, _: &ChannelTarget, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn reserve_text(
        &self,
        _: &ChannelTarget,
        text_id: Uuid,
    ) -> anyhow::Result<Option<TextIdentity>> {
        Ok((!self.legacy).then_some(TextIdentity {
            text_id,
            channel_instance_id: self.instance,
        }))
    }
    async fn deliver_text(
        &self,
        _: &ChannelTarget,
        identity: Option<TextIdentity>,
        mut input: TextPayloadStream,
        observer: Arc<dyn TextObserver>,
        cancel: CancellationToken,
    ) -> anyhow::Result<TextDeliveryReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        observer.start().await?;
        if let Some(identity) = identity {
            *self.progress.lock().unwrap() = Some(TextProgress {
                identity,
                receipt_sequence: 1,
                accepted_utf8_bytes: 0,
                written_utf8_bytes: Some(0),
                write_boundary: Some(TextWriteBoundary::StdoutWrite),
                end_accepted: false,
                observation_end: None,
            });
        }
        while let Some(frame) = input.next().await {
            if cancel.is_cancelled() {
                break;
            }
            match frame? {
                TextFrame::Chunk(chunk) => {
                    observer.offered(&chunk).await?;
                    if self.legacy {
                        self.native_bytes
                            .fetch_add(chunk.text.len(), Ordering::SeqCst);
                        continue;
                    }
                    {
                        let mut stored = self.progress.lock().unwrap();
                        let p = stored.as_mut().unwrap();
                        p.receipt_sequence += 1;
                        p.accepted_utf8_bytes += chunk.text.len() as u64;
                    }
                    let blocked = self.blocked.lock().unwrap().take();
                    if let Some(blocked) = blocked {
                        self.active_write.store(true, Ordering::SeqCst);
                        let active = self.active_write.clone();
                        let bytes = self.native_bytes.clone();
                        let progress = self.progress.clone();
                        let entered = self.entered.clone();
                        let changed = self.changed.clone();
                        let cancelled = self.cancel.clone();
                        // Its real native thread survives all async reply drops.
                        tokio::task::spawn_blocking(move || {
                            entered.add_permits(1);
                            blocked.recv().unwrap();
                            bytes.fetch_add(1, Ordering::SeqCst);
                            let mut stored = progress.lock().unwrap();
                            let p = stored.as_mut().unwrap();
                            p.receipt_sequence += 1;
                            p.written_utf8_bytes = Some(1);
                            if cancelled.load(Ordering::SeqCst) {
                                p.observation_end = Some(ObservationEnd::Cancelled {
                                    reason: "original native write joined".into(),
                                });
                            }
                            active.store(false, Ordering::SeqCst);
                            drop(stored);
                            changed.notify_waiters();
                        })
                        .await?;
                    } else {
                        self.native_bytes
                            .fetch_add(chunk.text.len(), Ordering::SeqCst);
                        let mut stored = self.progress.lock().unwrap();
                        let p = stored.as_mut().unwrap();
                        p.receipt_sequence += 1;
                        p.written_utf8_bytes = Some(p.accepted_utf8_bytes);
                    }
                    let p = self.progress.lock().unwrap().clone().unwrap();
                    observer.progress(p.clone()).await?;
                    if let Some(end) = p.observation_end.clone() {
                        return Ok(TextDeliveryReport {
                            progress: Some(p),
                            observation_end: end,
                        });
                    }
                }
                TextFrame::End { chunks, utf8_bytes } => {
                    observer.end(chunks, utf8_bytes).await?;
                    if self.legacy {
                        return Ok(TextDeliveryReport::unobserved(
                            "legacy empty ACK is not write evidence",
                        ));
                    }
                    let p = {
                        let mut stored = self.progress.lock().unwrap();
                        let p = stored.as_mut().unwrap();
                        p.receipt_sequence += 1;
                        p.end_accepted = true;
                        p.observation_end = Some(ObservationEnd::SubmissionFinished);
                        p.clone()
                    };
                    observer.progress(p.clone()).await?;
                    return Ok(TextDeliveryReport {
                        progress: Some(p),
                        observation_end: ObservationEnd::SubmissionFinished,
                    });
                }
            }
        }
        anyhow::bail!("producer stopped without End")
    }
    async fn cancel_text(&self, identity: TextIdentity, _: &str) -> anyhow::Result<TextProgress> {
        anyhow::ensure!(
            identity.channel_instance_id == self.instance,
            "foreign original writer"
        );
        self.cancel.store(true, Ordering::SeqCst);
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if !self.active_write.load(Ordering::SeqCst) {
                break;
            }
            changed.await;
        }
        let mut stored = self.progress.lock().unwrap();
        let p = stored
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("no original checkpoint"))?;
        anyhow::ensure!(p.identity == identity, "another writer identity");
        if p.observation_end.is_none() {
            p.receipt_sequence += 1;
            p.observation_end = Some(ObservationEnd::Cancelled {
                reason: "exact text writer sealed".into(),
            });
        }
        Ok(p.clone())
    }
}
fn input(response_id: Uuid, channel: Arc<Channel>, end: bool) -> TextDelivery {
    let mut frames = vec![Ok(TextFrame::Chunk(TextChunk {
        sequence: 0,
        utf8_offset: 0,
        text: "你好".into(),
    }))];
    if end {
        frames.push(Ok(TextFrame::End {
            chunks: 1,
            utf8_bytes: 6,
        }));
    }
    TextDelivery {
        response_id,
        output_key: "text:response".into(),
        content: json!({"representation":"plain_utf8"}),
        target: ChannelTarget {
            user_id: channel.owner.to_string(),
            voice: None,
        },
        channel,
        text: Box::pin(futures::stream::iter(frames)),
        cancel: CancellationToken::new(),
    }
}
async fn started(f: &Fixture) -> (Arc<DeliveryService>, ResponseRecord) {
    let service = DeliveryService::new(f.repo(), &CancellationToken::new());
    service.start().await.unwrap();
    let response = service.prepare(f.response().await).await.unwrap();
    (service, response)
}

#[tokio::test]
async fn text_is_durable_before_native_write_and_retry_never_dispatches_again() {
    with_db(|f| async move {
        let (service, response) = started(&f).await;
        let channel = Channel::new(f.user, None, false);
        let record = service
            .deliver_text(input(response.response_id, channel.clone(), true))
            .await
            .unwrap();
        assert_eq!(record.status, DeliveryStatus::Submitted);
        assert_eq!(record.text.as_ref().unwrap().offered_utf8_bytes, 6);
        assert_eq!(
            record
                .text
                .as_ref()
                .unwrap()
                .progress
                .as_ref()
                .unwrap()
                .written_utf8_bytes,
            Some(6)
        );
        assert_eq!(f.count("delivery_text_chunks").await, 1);
        let retry = service
            .deliver_text(input(response.response_id, channel.clone(), true))
            .await
            .unwrap();
        assert_eq!(retry.delivery_id, record.delivery_id);
        assert_eq!(channel.calls.load(Ordering::SeqCst), 1);
        assert_eq!(channel.native_bytes.load(Ordering::SeqCst), 6);
        assert!(
            service
                .text_chunks(Uuid::new_v4(), record.delivery_id, None, 64)
                .await
                .is_err()
        );
        assert!(
            service
                .text_chunks(f.user, record.delivery_id, None, 0)
                .await
                .is_err()
        );
        assert!(
            service
                .text_chunks(f.user, record.delivery_id, None, 257)
                .await
                .is_err()
        );
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn dropped_text_waiter_keeps_native_owner_until_exact_cancel_checkpoint() {
    with_db(|f| async move {
        let (service, response) = started(&f).await;
        let (release, blocked) = std::sync::mpsc::channel();
        let channel = Channel::new(f.user, Some(blocked), false);
        let running_service = service.clone();
        let running_channel = channel.clone();
        let waiter = tokio::spawn(async move {
            running_service
                .deliver_text(input(response.response_id, running_channel, true))
                .await
        });
        channel.entered.acquire().await.unwrap().forget();
        assert_eq!(
            f.count("delivery_text_chunks").await,
            1,
            "native write entered before its immutable chunk committed"
        );
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        let observed = service
            .deliver_text(input(response.response_id, channel.clone(), true))
            .await
            .unwrap();
        assert_eq!(observed.status, DeliveryStatus::Sending);
        assert_eq!(channel.calls.load(Ordering::SeqCst), 1);
        service
            .cancel_response(f.user, response.response_id)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !channel.cancel.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(channel.active_write.load(Ordering::SeqCst));
        assert_eq!(
            service
                .get_delivery(f.user, observed.delivery_id)
                .await
                .unwrap()
                .status,
            DeliveryStatus::Sending
        );
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let record = service
                    .get_delivery(f.user, observed.delivery_id)
                    .await
                    .unwrap();
                if record.status == DeliveryStatus::Cancelled {
                    let text = record.text.unwrap();
                    assert!(!text.end_offered);
                    assert_eq!(text.progress.unwrap().written_utf8_bytes, Some(1));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        service.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn failed_text_offer_never_reaches_native_and_error_eof_never_completes() {
    with_db(|f| async move {
        let (service, response) = started(&f).await;
        f.sql("CREATE FUNCTION reject_text_offer() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected text durable offer failure'; END $$; CREATE TRIGGER reject_text_offer BEFORE INSERT ON delivery_text_chunks FOR EACH ROW EXECUTE FUNCTION reject_text_offer()").await;
        let channel = Channel::new(f.user, None, false);
        let record = service.deliver_text(input(response.response_id, channel.clone(), true)).await.unwrap();
        assert_eq!(channel.native_bytes.load(Ordering::SeqCst), 0);
        assert_eq!(record.text.unwrap().offered_utf8_bytes, 0);
        assert_ne!(record.status, DeliveryStatus::Submitted);
        f.sql("DROP TRIGGER reject_text_offer ON delivery_text_chunks").await;
        let response = service.prepare(f.response().await).await.unwrap();
        let channel = Channel::new(f.user, None, false);
        let record = service.deliver_text(input(response.response_id, channel.clone(), false)).await.unwrap();
        assert_eq!(channel.native_bytes.load(Ordering::SeqCst), 6);
        assert!(!record.text.unwrap().end_offered);
        assert_ne!(record.status, DeliveryStatus::Submitted);
        service.shutdown().await.unwrap();
    }).await;
}

#[tokio::test]
async fn legacy_ack_is_unknown_without_invented_written_bytes() {
    with_db(|f| async move {
        let (service, response) = started(&f).await;
        let channel = Channel::new(f.user, None, true);
        let record = service
            .deliver_text(input(response.response_id, channel.clone(), true))
            .await
            .unwrap();
        assert_eq!(record.status, DeliveryStatus::Unknown);
        let text = record.text.unwrap();
        assert!(text.identity.is_none());
        assert!(text.progress.is_none());
        assert!(text.end_offered);
        service
            .deliver_text(input(response.response_id, channel.clone(), true))
            .await
            .unwrap();
        assert_eq!(channel.calls.load(Ordering::SeqCst), 1);
        service.shutdown().await.unwrap();
    })
    .await;
}
