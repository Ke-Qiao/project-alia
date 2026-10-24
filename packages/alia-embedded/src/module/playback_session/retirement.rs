//! Exact original-owner facts survive replacement. Unknown is never zero output.
use super::*;
use alia_core::output_control::{
    OutputRegistration, OutputRetirementEvidence, OutputRetirementReceipt, OutputRetirementRequest,
};

pub(super) const RETIRED_CAPACITY: usize = 64;

#[derive(Clone)]
pub(super) struct RetiredOutput {
    pub capture: Option<CaptureControlBarrier>,
    pub scope: VoiceScope,
    pub registration: Option<OutputRegistration>,
    pub control: Option<Arc<PlaybackControl>>,
    pub generation: Option<u64>,
    pub observations: HashMap<PlaybackIdentity, PlaybackLease>,
    pub stop_error: Option<String>,
}
impl RetiredOutput {
    pub(super) fn quiescent(&self) -> bool {
        self.control
            .as_ref()
            .is_none_or(|control| control.quiescent())
            && self
                .observations
                .values()
                .all(|lease| lease.submission.output_quiescent())
    }
    fn matches(&self, request: &OutputRetirementRequest) -> Result<()> {
        ensure!(self.scope == request.scope, OutputControlError::Unknown);
        if let Some(registered) = &self.registration {
            ensure!(
                request.binding == Some(registered.binding),
                OutputControlError::Conflict
            );
            ensure!(
                request
                    .expected_backend
                    .as_ref()
                    .is_none_or(|backend| backend == &registered.backend),
                OutputControlError::Conflict
            );
        } else {
            ensure!(
                self.observations.values().all(|lease| lease.local_cue)
                    && request.expected_backend.is_none(),
                "unregistered scope cannot contain remote Response media"
            );
        }
        Ok(())
    }
}

impl PlaybackSession {
    pub(super) fn remember_retirement_error(
        &self,
        scope: VoiceScope,
        kind: &str,
        result: &Result<()>,
    ) {
        if let Err(error) = result {
            let message = format!("{kind} retirement for {scope:?}: {error}");
            record_task_error(&self.task_errors, message.clone());
            if let Some(original) = self
                .retired
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter_mut()
                .find(|owner| owner.scope == scope)
            {
                original.stop_error.get_or_insert(message);
            }
        }
    }
    /// Called under the Session lock before opening any new local epoch. A live
    /// epoch reserves the last slot so retirement never drops an unsettled owner.
    pub(super) fn reserve_retirement_slot(&self) -> Result<()> {
        let mut retired = self.retired.lock().unwrap_or_else(|e| e.into_inner());
        if retired.len() >= RETIRED_CAPACITY {
            let reusable = retired
                .iter()
                .position(RetiredOutput::quiescent)
                .context("retired output capacity contains only unsettled owners")?;
            retired.remove(reusable);
        }
        Ok(())
    }

    /// Authenticated exact retirement only. It cannot activate another epoch.
    pub fn retire_output(&self, request: &OutputRetirementRequest) -> Result<()> {
        request.validate()?;
        let mut state = self.lock()?;
        if let Some(active) = state
            .as_mut()
            .filter(|active| active.scope == request.scope && !active.retired)
        {
            if let Some(registered) = &active.registration {
                ensure!(
                    request.binding == Some(registered.binding),
                    OutputControlError::Conflict
                );
                ensure!(
                    request
                        .expected_backend
                        .as_ref()
                        .is_none_or(|backend| backend == &registered.backend),
                    OutputControlError::Conflict
                );
            } else {
                ensure!(
                    active.observations.values().all(|lease| lease.local_cue)
                        && request.expected_backend.is_none(),
                    "unregistered scope cannot contain remote Response media"
                );
            }
            let motion = match &self.output_fence {
                Some(sink) => match request.scope.generation_epoch.checked_add(1) {
                    Some(epoch) => sink.fence_voice(&VoiceScope {
                        generation_epoch: epoch,
                        ..request.scope
                    }),
                    None => Err(anyhow::anyhow!(
                        "voice epoch exhausted during motion retirement"
                    )),
                },
                None => Ok(()),
            };
            let audio = self.retire(active, "original output explicitly retired");
            self.remember_retirement_error(request.scope, "motion", &motion);
            motion.and(audio)?;
        }
        // A stale request must use retained facts, never the replacement scope.
        let retired = self.retired.lock().unwrap_or_else(|e| e.into_inner());
        let original = retired
            .iter()
            .find(|owner| owner.scope == request.scope)
            .ok_or(OutputControlError::Unknown)?;
        original.matches(request)?;
        if let Some(error) = &original.stop_error {
            anyhow::bail!("original native retirement failed: {error}");
        }
        Ok(())
    }

    /// No deadline drops the original owner. Callers may lose the RPC, while
    /// this bounded record still fences admission and retains entered writes.
    pub async fn observe_output_retirement(
        &self,
        request: &OutputRetirementRequest,
    ) -> Result<OutputRetirementReceipt> {
        request.validate()?;
        let original = {
            let retired = self.retired.lock().unwrap_or_else(|e| e.into_inner());
            retired
                .iter()
                .find(|owner| owner.scope == request.scope)
                .cloned()
                .ok_or(OutputControlError::Unknown)?
        };
        original.matches(request)?;
        if let Some(error) = &original.stop_error {
            anyhow::bail!("original native retirement failed: {error}");
        }
        while !original.quiescent() {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let evidence = match &original.control {
            Some(control) => {
                ensure!(
                    original
                        .control
                        .as_ref()
                        .is_some_and(|control| control.is_retired()),
                    "original native gate was not retired"
                );
                let mut leases: Vec<_> = original
                    .observations
                    .values()
                    .filter(|lease| !lease.local_cue)
                    .collect();
                leases.sort_by_key(|lease| lease.playback_sequence);
                OutputRetirementEvidence::Settled {
                    backend: control.backend.clone(),
                    progress: leases
                        .into_iter()
                        .map(|lease| lease.submission.next_snapshot())
                        .collect(),
                }
            }
            None => {
                ensure!(
                    original.generation.is_none() && original.observations.is_empty(),
                    "missing control cannot prove no native owner"
                );
                OutputRetirementEvidence::NoNativeOwner
            }
        };
        let receipt = OutputRetirementReceipt {
            scope: request.scope,
            binding: request.binding,
            evidence,
        };
        receipt.validate(request)?;
        Ok(receipt)
    }

    pub(super) fn retired_lease(&self, identity: &PlaybackIdentity) -> Option<PlaybackLease> {
        self.retired
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find_map(|owner| owner.observations.get(identity).cloned())
    }

    pub(super) async fn wait_retired_writes(&self) {
        loop {
            if self
                .retired
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .all(RetiredOutput::quiescent)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    }
}
