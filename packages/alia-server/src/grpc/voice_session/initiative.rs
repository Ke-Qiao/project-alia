//! Opportunity checks execute inside the capture actor, never from a cached
//! presence event. Database work does not own capture; drain after every await.
use std::{collections::HashSet, time::SystemTime};

use super::*;
use crate::modules::{
    interaction::initiative::{
        Request, ReservedVoiceInitiative, VoiceInitiativeRequest, VoiceOpportunity,
    },
    memory::context::ContextUse,
};
use chrono::Utc;

const CAPTURE_FRESHNESS: Duration = Duration::from_secs(2);
const OPPORTUNITY_TTL: Duration = Duration::from_secs(5);
const MAX_RESERVATIONS: usize = 64;

async fn drain<S: Stream<Item = Result<DeviceFrame, Status>> + Unpin>(
    stream: &mut S,
    window: &mut CaptureWindow,
    credentials: &mut SessionCredentials,
    output: Option<&VoiceOutput>,
    lease: &SessionLease,
) -> anyhow::Result<()> {
    match drain_pending(stream, window, credentials, output).await {
        Ok(true) => Ok(()),
        result => {
            lease.cancel.cancel();
            match result {
                Err(error) => Err(error.into()),
                _ => anyhow::bail!("voice capture closed"),
            }
        }
    }
}

async fn revalidate(credentials: &SessionCredentials, lease: &SessionLease) -> anyhow::Result<()> {
    if let Err(error) = credentials.revalidate().await {
        lease.cancel.cancel();
        return Err(error.into());
    }
    Ok(())
}

pub(super) struct Session {
    pub receiver: mpsc::Receiver<Request>,
    _registration: crate::modules::interaction::initiative::Registration,
    context: Arc<ContextService>,
    boot: Uuid,
    opportunity: Option<(VoiceOpportunity, tokio::time::Instant)>,
    used_requests: HashSet<Uuid>,
    used_initiatives: HashSet<Uuid>,
    active: Option<CancellationToken>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(cancel) = &self.active {
            cancel.cancel();
        }
    }
}

impl Session {
    pub fn new(
        handle: VoiceInitiativeHandle,
        context: Arc<ContextService>,
        lease: &SessionLease,
        boot: Uuid,
    ) -> Self {
        let (registration, receiver) = handle.register(lease.channel.clone(), lease.id);
        Self {
            receiver,
            _registration: registration,
            context,
            boot,
            opportunity: None,
            used_requests: HashSet::new(),
            used_initiatives: HashSet::new(),
            active: None,
        }
    }

    fn available(
        &self,
        lease: &SessionLease,
        window: &CaptureWindow,
        busy: bool,
        output: Option<&VoiceOutput>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !lease.cancel.is_cancelled(),
            "original voice session closed"
        );
        anyhow::ensure!(
            !busy && window.is_idle(),
            "user utterance or turn detection is active"
        );
        anyhow::ensure!(
            output.is_none_or(|output| !output.is_alive()),
            "original output is still active or held"
        );
        let received = window
            .last_received_at()
            .ok_or_else(|| anyhow::anyhow!("voice capture has no verified frame"))?;
        anyhow::ensure!(
            SystemTime::now()
                .duration_since(received)
                .is_ok_and(|age| age <= CAPTURE_FRESHNESS),
            "voice capture is stale"
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn handle<S: Stream<Item = Result<DeviceFrame, Status>> + Unpin>(
        &mut self,
        request: Request,
        stream: &mut S,
        tx: &mpsc::Sender<Result<SessionEvent, Status>>,
        lease: &SessionLease,
        credentials: &mut SessionCredentials,
        window: &mut CaptureWindow,
        controller: &mut TurnController,
        busy: bool,
        output: &mut Option<VoiceOutput>,
        response_cancel: &mut CancellationToken,
        runtime: &OutputRuntime,
    ) {
        match request {
            Request::Snapshot { owner, reply } => {
                if reply.is_closed() {
                    return;
                }
                let result = async {
                    drain(stream, window, credentials, output.as_ref(), lease).await?;
                    revalidate(credentials, lease).await?;
                    drain(stream, window, credentials, output.as_ref(), lease).await?;
                    let current_owner = lease.slot.owner.lock().await;
                    drain(stream, window, credentials, output.as_ref(), lease).await?;
                    anyhow::ensure!(
                        current_owner
                            .as_ref()
                            .is_some_and(|(id, _)| *id == lease.id),
                        "original voice session replaced"
                    );
                    self.available(lease, window, busy, output.as_ref())?;
                    anyhow::ensure!(
                        credentials
                            .source
                            .as_ref()
                            .is_some_and(|source| source.user_id == owner),
                        "voice opportunity owner differs"
                    );
                    let scope = VoiceScope {
                        session_id: lease.id,
                        device_boot_id: self.boot,
                        generation_epoch: controller.generation_epoch(),
                    };
                    let observed_at = Utc::now();
                    let snapshot = VoiceOpportunity {
                        id: Uuid::new_v4(),
                        owner,
                        channel_id: lease.channel.clone(),
                        scope,
                        speech_revision: window.revision(),
                        through_sequence: window.through_sequence(),
                        observed_at,
                        last_capture_at: window
                            .last_received_at()
                            .expect("availability checked")
                            .into(),
                        expires_at: observed_at + chrono::Duration::from_std(OPPORTUNITY_TTL)?,
                    };
                    self.opportunity = Some((
                        snapshot.clone(),
                        tokio::time::Instant::now() + OPPORTUNITY_TTL,
                    ));
                    Ok(snapshot)
                }
                .await;
                let _ = reply.send(result);
            }
            Request::Reserve { request, reply } => {
                if reply.is_closed() || request.cancel.is_cancelled() {
                    request.cancel.cancel();
                    return;
                }
                let result = self
                    .reserve(
                        request,
                        stream,
                        tx,
                        lease,
                        credentials,
                        window,
                        controller,
                        busy,
                        output,
                        response_cancel,
                        runtime,
                    )
                    .await;
                if let Err(result) = reply.send(result) {
                    if let Ok(admitted) = result {
                        admitted.cancel.cancel();
                    }
                }
            }
        }
    }

    fn check_request(
        &self,
        request: &VoiceInitiativeRequest,
        lease: &SessionLease,
        window: &CaptureWindow,
        controller: &TurnController,
        busy: bool,
        output: Option<&VoiceOutput>,
    ) -> anyhow::Result<()> {
        self.available(lease, window, busy, output)?;
        anyhow::ensure!(!request.cancel.is_cancelled(), "initiative cancelled");
        let (original, monotonic_deadline) = self
            .opportunity
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("voice opportunity absent or consumed"))?;
        anyhow::ensure!(
            original == &request.opportunity
                && tokio::time::Instant::now() < *monotonic_deadline
                && Utc::now() < original.expires_at
                && Utc::now() < request.deadline,
            "voice opportunity changed or expired"
        );
        anyhow::ensure!(
            original.scope.session_id == lease.id
                && original.scope.generation_epoch == controller.generation_epoch()
                && original.scope.device_boot_id == self.boot
                && original.channel_id == lease.channel
                && original.speech_revision == window.revision(),
            "capture or original session superseded opportunity"
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn reserve<S: Stream<Item = Result<DeviceFrame, Status>> + Unpin>(
        &mut self,
        request: VoiceInitiativeRequest,
        stream: &mut S,
        tx: &mpsc::Sender<Result<SessionEvent, Status>>,
        lease: &SessionLease,
        credentials: &mut SessionCredentials,
        window: &mut CaptureWindow,
        controller: &mut TurnController,
        busy: bool,
        output: &mut Option<VoiceOutput>,
        response_cancel: &mut CancellationToken,
        runtime: &OutputRuntime,
    ) -> anyhow::Result<ReservedVoiceInitiative> {
        let guard = request.cancel.clone().drop_guard();
        // Shadow wiring alone cannot authorize output. CP05 must connect the
        // dedicated live policy/source grant and atomic device capture-watermark
        // admission before this gate is backed by persistent authorization.
        // No production builder can enable it in this intermediate package.
        anyhow::ensure!(
            runtime.initiative_output_enabled,
            "initiative output admission is disabled"
        );
        anyhow::ensure!(
            !request.initiative_id.is_nil()
                && !request.context.request_id.is_nil()
                && !request.context.context_id.is_nil()
                && request.context.owner_id == request.opportunity.owner,
            "invalid initiative identity"
        );
        anyhow::ensure!(
            self.used_requests.len() < MAX_RESERVATIONS,
            "voice initiative reservation capacity exhausted"
        );
        anyhow::ensure!(
            !self.used_requests.contains(&request.context.request_id)
                && !self.used_initiatives.contains(&request.initiative_id),
            "initiative reservation already attempted; reconcile original identity"
        );
        self.used_requests.insert(request.context.request_id);
        self.used_initiatives.insert(request.initiative_id);
        drain(stream, window, credentials, output.as_ref(), lease).await?;
        self.check_request(&request, lease, window, controller, busy, output.as_ref())?;
        let validated = self
            .context
            .validate(&request.context, ContextUse::OutputAdmission)
            .await?;
        drain(stream, window, credentials, output.as_ref(), lease).await?;
        self.check_request(&request, lease, window, controller, busy, output.as_ref())?;
        revalidate(credentials, lease).await?;
        drain(stream, window, credentials, output.as_ref(), lease).await?;
        let source = credentials
            .source
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("voice source absent"))?;
        anyhow::ensure!(
            source.user_id == request.context.owner_id
                && source.privacy_epoch == request.context.source_privacy_epoch
                && validated.lease.source().channel_id == lease.channel,
            "initiative differs from original voice source"
        );
        let channel = source.channel();
        let deadline = request
            .deadline
            .min(request.opportunity.expires_at)
            .min(validated.lease.valid_until().unwrap_or(request.deadline));
        let current_owner = lease.slot.owner.lock().await;
        drain(stream, window, credentials, output.as_ref(), lease).await?;
        anyhow::ensure!(
            current_owner
                .as_ref()
                .is_some_and(|(id, _)| *id == lease.id),
            "original voice session replaced"
        );
        self.check_request(&request, lease, window, controller, busy, output.as_ref())?;
        // From this point a failed acknowledgement closes this session rather
        // than letting its new epoch masquerade as an unconsumed opportunity.
        let session_guard = lease.cancel.clone().drop_guard();
        let scope = VoiceScope {
            session_id: lease.id,
            device_boot_id: self.boot,
            generation_epoch: controller.invalidate()?,
        };
        self.opportunity = None;
        fence_output(&channel, scope, false, "initiative_output_admitted").await?;
        drain(stream, window, credentials, output.as_ref(), lease).await?;
        self.available(lease, window, busy, output.as_ref())?;
        anyhow::ensure!(
            window.revision() == request.opportunity.speech_revision
                && !request.cancel.is_cancelled()
                && Utc::now() < deadline,
            "initiative opportunity expired during fence"
        );
        let producer = runtime.delivery.begin_voice_output(
            request.context.owner_id,
            request.context.request_id,
            scope,
            channel,
            request.cancel.clone(),
            Some(runtime.hold_budget),
        )?;
        producer.constrain_initiative(request.context.context_id, deadline)?;
        *response_cancel = request.cancel.clone();
        self.active = Some(request.cancel.clone());
        *output = Some(producer.output());
        send_event(
            tx,
            lease,
            self.boot,
            scope.generation_epoch,
            session_event::Payload::InitiativeAdmitted(voice::InitiativeAdmitted {
                initiative_id: request.initiative_id.to_string(),
                request_id: request.context.request_id.to_string(),
                context_id: request.context.context_id.to_string(),
                through_sequence: window.through_sequence(),
            }),
        )
        .await?;
        // Capture can arrive while the event queue is backpressured. The newly
        // installed original output receives its hold before the caller runs.
        drain(stream, window, credentials, output.as_ref(), lease).await?;
        anyhow::ensure!(
            !request.cancel.is_cancelled() && Utc::now() < deadline,
            "initiative expired before acknowledgement"
        );
        drop(current_owner);
        guard.disarm();
        session_guard.disarm();
        Ok(ReservedVoiceInitiative {
            initiative_id: request.initiative_id,
            context: request.context,
            output: producer,
            scope,
            cancel: request.cancel,
            deadline,
        })
    }
}
