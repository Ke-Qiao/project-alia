//! Bounded requests to the original live voice actor. A snapshot is evidence,
//! never permission to allocate a generation or send media.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::modules::{delivery::OutputProducer, memory::context::ContextBinding};

const QUEUE_CAPACITY: usize = 4;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceOpportunity {
    pub owner: Uuid,
    pub channel_id: String,
    pub scope: alia_core::delivery::VoiceScope,
    pub speech_revision: u64,
    pub through_sequence: u64,
    pub observed_at: DateTime<Utc>,
    pub last_capture_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    // Only the actor can mint a snapshot; public identity fields are rechecked.
    pub(crate) id: Uuid,
}

#[derive(Debug)]
pub(crate) struct VoiceInitiativeRequest {
    pub(crate) permit: crate::modules::companion::live::LiveAdmissionPermit,
    pub(crate) cancel: CancellationToken,
}
impl VoiceInitiativeRequest {
    pub(crate) fn from_permit(
        permit: crate::modules::companion::live::LiveAdmissionPermit,
        cancel: CancellationToken,
    ) -> Self {
        Self { permit, cancel }
    }
}

#[derive(Debug)]
pub struct ReservedVoiceInitiative {
    pub initiative_id: Uuid,
    pub context: ContextBinding,
    pub output: Arc<OutputProducer>,
    pub scope: alia_core::delivery::VoiceScope,
    pub cancel: CancellationToken,
    pub deadline: DateTime<Utc>,
}

pub(crate) enum Request {
    Snapshot {
        owner: Uuid,
        reply: oneshot::Sender<anyhow::Result<VoiceOpportunity>>,
    },
    Reserve {
        request: VoiceInitiativeRequest,
        reply: oneshot::Sender<anyhow::Result<ReservedVoiceInitiative>>,
    },
}

struct Endpoint {
    session: Uuid,
    sender: mpsc::Sender<Request>,
}

#[derive(Clone)]
pub struct VoiceInitiativeHandle(Arc<Mutex<HashMap<String, Endpoint>>>, Uuid);
impl Default for VoiceInitiativeHandle {
    fn default() -> Self {
        Self(Arc::default(), Uuid::new_v4())
    }
}
impl VoiceInitiativeHandle {
    pub(crate) fn instance_id(&self) -> Uuid {
        self.1
    }

    pub async fn snapshot(
        &self,
        owner: Uuid,
        channel_id: &str,
    ) -> anyhow::Result<VoiceOpportunity> {
        anyhow::ensure!(!owner.is_nil(), "initiative owner is absent");
        let sender = self.sender(channel_id, None)?;
        let (reply, receive) = oneshot::channel();
        sender
            .try_send(Request::Snapshot { owner, reply })
            .map_err(|_| anyhow::anyhow!("voice opportunity mailbox unavailable"))?;
        tokio::time::timeout(REQUEST_TIMEOUT, receive)
            .await
            .map_err(|_| anyhow::anyhow!("voice opportunity snapshot timed out"))?
            .map_err(|_| anyhow::anyhow!("original voice session ended"))?
    }

    /// Cancellation on drop covers unknown acknowledgements too. A caller must
    /// reconcile its durable initiative instead of submitting a new identity.
    pub(crate) async fn reserve(
        &self,
        request: VoiceInitiativeRequest,
    ) -> anyhow::Result<ReservedVoiceInitiative> {
        let guard = request.cancel.clone().drop_guard();
        let sender = self.sender(
            &request.permit.opportunity().channel_id,
            Some(request.permit.opportunity().scope.session_id),
        )?;
        let (reply, receive) = oneshot::channel();
        sender
            .try_send(Request::Reserve { request, reply })
            .map_err(|_| anyhow::anyhow!("voice initiative mailbox unavailable"))?;
        let admitted = tokio::time::timeout(REQUEST_TIMEOUT, receive)
            .await
            .map_err(|_| anyhow::anyhow!("voice initiative acknowledgement unknown"))?
            .map_err(|_| anyhow::anyhow!("original voice session ended"))??;
        guard.disarm();
        Ok(admitted)
    }

    fn sender(
        &self,
        channel: &str,
        session: Option<Uuid>,
    ) -> anyhow::Result<mpsc::Sender<Request>> {
        let sessions = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let endpoint = sessions
            .get(channel)
            .ok_or_else(|| anyhow::anyhow!("no active voice opportunity"))?;
        anyhow::ensure!(
            session.is_none_or(|id| id == endpoint.session),
            "voice opportunity session was replaced"
        );
        Ok(endpoint.sender.clone())
    }

    pub(crate) fn register(
        &self,
        channel: String,
        session: Uuid,
    ) -> (Registration, mpsc::Receiver<Request>) {
        let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(channel.clone(), Endpoint { session, sender });
        (
            Registration {
                handle: self.clone(),
                channel,
                session,
            },
            receiver,
        )
    }
}

pub(crate) struct Registration {
    handle: VoiceInitiativeHandle,
    channel: String,
    session: Uuid,
}
impl Drop for Registration {
    fn drop(&mut self) {
        let mut sessions = self
            .handle
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if sessions
            .get(&self.channel)
            .is_some_and(|endpoint| endpoint.session == self.session)
        {
            sessions.remove(&self.channel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(session: Uuid) -> VoiceInitiativeRequest {
        let owner = Uuid::new_v4();
        let opportunity = VoiceOpportunity {
            id: Uuid::new_v4(),
            owner,
            channel_id: "device".into(),
            scope: alia_core::delivery::VoiceScope {
                session_id: session,
                device_boot_id: Uuid::new_v4(),
                generation_epoch: 1,
            },
            speech_revision: 0,
            through_sequence: 0,
            observed_at: Utc::now(),
            last_capture_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::seconds(5),
        };
        let context = ContextBinding {
            context_id: Uuid::new_v4(),
            owner_id: owner,
            request_id: Uuid::new_v4(),
            source_event_id: Some(Uuid::new_v4()),
            source_privacy_epoch: 1,
        };
        VoiceInitiativeRequest::from_permit(
            crate::modules::companion::live::LiveAdmissionPermit::fixture(
                opportunity,
                context,
                Utc::now() + chrono::Duration::seconds(5),
            ),
            CancellationToken::new(),
        )
    }

    #[tokio::test]
    async fn dropping_reservation_waiter_cancels_original_request() {
        let handle = VoiceInitiativeHandle::default();
        let session = Uuid::new_v4();
        let (_registration, mut receiver) = handle.register("device".into(), session);
        let input = request(session);
        let cancel = input.cancel.clone();
        let waiter = tokio::spawn(async move { handle.reserve(input).await });
        let Request::Reserve {
            request,
            reply: _reply,
        } = receiver.recv().await.unwrap()
        else {
            panic!("expected reservation")
        };
        assert!(!request.cancel.is_cancelled());
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(cancel.is_cancelled());
    }

    #[tokio::test]
    async fn replacement_keeps_new_mailbox_and_rejects_old_scope() {
        let handle = VoiceInitiativeHandle::default();
        let old = Uuid::new_v4();
        let (old_registration, _old_receiver) = handle.register("device".into(), old);
        let current = Uuid::new_v4();
        let (_registration, _receiver) = handle.register("device".into(), current);
        drop(old_registration);
        assert!(handle.sender("device", Some(current)).is_ok());
        let input = request(old);
        let cancel = input.cancel.clone();
        assert!(handle.reserve(input).await.is_err());
        assert!(cancel.is_cancelled());
        assert!(handle.sender("device", Some(current)).is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn reservation_timeout_cancels_original_without_enqueuing_a_retry() {
        let handle = VoiceInitiativeHandle::default();
        let session = Uuid::new_v4();
        let (_registration, mut receiver) = handle.register("device".into(), session);
        let input = request(session);
        let cancel = input.cancel.clone();
        let waiter = tokio::spawn(async move { handle.reserve(input).await });
        let Request::Reserve { request, reply } = receiver.recv().await.unwrap() else {
            panic!("expected reservation")
        };
        assert!(!request.cancel.is_cancelled());
        tokio::time::advance(REQUEST_TIMEOUT).await;
        let error = waiter.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("acknowledgement unknown"));
        assert!(cancel.is_cancelled());
        assert!(request.cancel.is_cancelled());
        assert!(reply.is_closed());
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
}
