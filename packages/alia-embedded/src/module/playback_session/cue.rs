//! Local cues own media identities; they share only the scope's native gate.
use super::*;

impl PlaybackSession {
    pub fn start_cue(&self, scope: &VoiceScope, sample_rate: u32) -> Result<PlaybackLease> {
        let _gate = self.task_gate.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(!self.shutdown.is_cancelled(), "playback runtime is stopped");
        ensure!(
            (8000..=192000).contains(&sample_rate),
            "invalid cue sample rate"
        );
        let mut state = self.lock()?;
        let active = state
            .as_mut()
            .filter(|a| a.scope == *scope && !a.retired)
            .context("cue scope is not locally current")?;
        ensure!(
            active.initiative_event.is_none(),
            "local cue cannot precede an initiative"
        );
        let capture = active
            .capture
            .as_ref()
            .context("cue requires a live capture owner")?
            .watermarks()?;
        active
            .observations
            .retain(|_, lease| lease.submission.is_live() || !lease.submission.output_quiescent());
        ensure!(
            active.observations.len() < 32,
            "playback observation capacity reached"
        );
        ensure!(
            !active
                .observations
                .values()
                .any(|l| l.local_cue && l.submission.is_live()),
            "a cue is already active"
        );
        let uninitialized = active.control.is_none();
        let generation = self.control_generation(active)?;
        let control = self.backend.playback_control(scope, generation)?;
        if uninitialized {
            control.initialize_capture(capture.0, capture.1)?;
        }
        active.control = Some(control.clone());
        control.snapshot(); // Expiry can retire a held owner even without another command.
        ensure!(
            !control.is_held() && !control.is_retired(),
            "cue output is held or retired"
        );
        // UUID distinguishes cues; this local sequence never advances the server's counter.
        let identity = PlaybackIdentity {
            scope: *scope,
            playback_id: Uuid::new_v4(),
            playback_sequence: 1,
        };
        let submission = self
            .backend
            .open_cue_submission(identity, sample_rate, generation)?;
        let lease = PlaybackLease {
            scope: *scope,
            playback_id: identity.playback_id.to_string(),
            playback_sequence: identity.playback_sequence,
            sample_rate,
            submission,
            generation,
            local_cue: true,
        };
        active.observations.insert(identity, lease.clone());
        Ok(lease)
    }

    pub fn enqueue_cue(&self, lease: &PlaybackLease, pcm: &[i16]) -> Result<bool> {
        ensure!(lease.local_cue, "expected a local cue identity");
        if !self.is_current(lease)? {
            return Ok(false);
        }
        self.backend
            .submit_audio(&lease.submission, lease.submission.accepted(), pcm)
    }

    pub fn end_cue(&self, lease: &PlaybackLease) -> Result<bool> {
        ensure!(lease.local_cue, "expected a local cue identity");
        if !self.is_current(lease)? {
            return Ok(false);
        }
        self.backend
            .end_submission(&lease.submission, lease.submission.accepted())
    }

    pub fn cancel_cue(&self, lease: &PlaybackLease, reason: &str) -> Result<bool> {
        ensure!(lease.local_cue, "expected a local cue identity");
        self.cancel_identity(&lease.identity(), reason)
    }
}
