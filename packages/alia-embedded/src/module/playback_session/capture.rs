//! Capture admission and resume share the Session mutex. Upload success is not
//! the capture clock: even queued frames must fence an older semantic decision.
use super::PlaybackSession;
use anyhow::{Result, ensure};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct CaptureControlBarrier(Arc<Mutex<Watermarks>>);
#[derive(Default)]
struct Watermarks {
    through: Option<u64>,
    speech: Option<u64>,
    retired: bool,
}
impl CaptureControlBarrier {
    pub(super) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub(super) fn watermarks(&self) -> Result<(Option<u64>, Option<u64>)> {
        let state = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("capture lock poisoned"))?;
        ensure!(!state.retired, "capture owner has been retired");
        Ok((state.through, state.speech))
    }
    pub(super) fn mark_retired(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).retired = true;
    }
}
impl PlaybackSession {
    /// `enqueue` must be a nonblocking local try_send. It runs after the real
    /// capture watermark changes, before releasing the resume admission mutex.
    pub fn accept_capture<T>(
        &self,
        capture: &CaptureControlBarrier,
        sequence: u64,
        speech: bool,
        enqueue: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let mut state = self.lock()?;
        let active = state.as_mut().filter(|active| {
            active
                .capture
                .as_ref()
                .is_some_and(|owner| owner.same_owner(capture))
        });
        let result = (|| {
            ensure!(!self.shutdown.is_cancelled(), "playback runtime stopped");
            let mut clock = capture
                .0
                .lock()
                .map_err(|_| anyhow::anyhow!("capture lock poisoned"))?;
            ensure!(!clock.retired, "capture owner retired");
            ensure!(
                match clock.through {
                    None => sequence == 0,
                    Some(previous) => previous.checked_add(1) == Some(sequence),
                },
                "capture sequence lost continuity"
            );
            clock.through = Some(sequence);
            if speech {
                clock.speech = Some(sequence);
            }
            if let Some(control) = active.as_ref().and_then(|active| active.control.as_ref()) {
                // An expired output cannot return, but capture may admit a
                // genuinely new generation under the same continuous session.
                if control.snapshot().phase != alia_core::playback_control::ControlPhase::Retired {
                    control.observe_capture(sequence, speech)?;
                }
            }
            enqueue()
        })();
        if result.is_err() {
            capture.mark_retired();
            if let Some(active) = active {
                self.retire(active, "capture continuity or upload ownership lost")?;
            }
        }
        result
    }
    /// Close only the Session owned by this actual capture handle. Wire scope
    /// equality alone cannot authorize cleanup after binding replacement.
    pub fn close_capture(&self, capture: &CaptureControlBarrier) -> Result<bool> {
        let mut state = self.lock()?;
        capture.mark_retired();
        if !state.as_ref().is_some_and(|active| {
            active
                .capture
                .as_ref()
                .is_some_and(|owner| owner.same_owner(capture))
        }) {
            return Ok(false);
        }
        let mut retired = state.take().expect("matching capture owner checked");
        let motion = self
            .output_fence
            .as_ref()
            .map_or(Ok(()), |sink| sink.close_voice(&retired.scope));
        let audio = self.retire(&mut retired, "voice capture owner closed");
        self.remember_retirement_error(retired.scope, "motion", &motion);
        motion.and(audio)?;
        Ok(true)
    }
    pub fn retire_capture(&self, capture: &CaptureControlBarrier) -> Result<()> {
        let mut state = self.lock()?;
        capture.mark_retired();
        if let Some(active) = state.as_mut().filter(|active| {
            active
                .capture
                .as_ref()
                .is_some_and(|owner| owner.same_owner(capture))
        }) {
            self.retire(active, "capture owner closed")?;
        }
        Ok(())
    }

    /// Call after the capture worker is joined. Original owner handles survive
    /// scope replacement; this observes native submission, not acoustic silence.
    pub async fn wait_capture_retired(&self, capture: &CaptureControlBarrier) -> Result<()> {
        let retired: Vec<_> = self
            .retired
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .filter(|owner| {
                owner
                    .capture
                    .as_ref()
                    .is_some_and(|original| original.same_owner(capture))
            })
            .cloned()
            .collect();
        tokio::time::timeout(std::time::Duration::from_millis(1500), async {
            for owner in retired {
                if let Some(error) = &owner.stop_error {
                    anyhow::bail!("original capture output retirement failed: {error}");
                }
                while !owner.quiescent() {
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| anyhow::anyhow!("original capture output writes did not settle"))?
    }
}
