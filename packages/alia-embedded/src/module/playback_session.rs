//! Network playback admission. Only the local voice-stream handshake activates
//! a session; playback RPCs may advance its epoch but cannot replace its identity.
use std::sync::{Arc, Mutex, MutexGuard};

use alia_core::delivery::VoiceScope;
use anyhow::{Context, Result, ensure};
use uuid::Uuid;

use super::traits::AudioPlaybackBackend;

#[derive(Clone, Debug)]
pub struct PlaybackLease {
    pub scope: VoiceScope,
    pub playback_id: String,
    pub playback_sequence: u64,
    pub sample_rate: u32,
    generation: u64,
}

struct ActiveSession {
    scope: VoiceScope,
    highest_sequence: Option<u64>,
    generation: Option<u64>,
    current: Option<PlaybackLease>,
    input_ended: bool,
}

pub struct PlaybackSession {
    backend: Arc<dyn AudioPlaybackBackend>,
    state: Mutex<Option<ActiveSession>>,
}

impl PlaybackSession {
    pub fn new(backend: Arc<dyn AudioPlaybackBackend>) -> Self {
        Self {
            backend,
            state: Mutex::new(None),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, Option<ActiveSession>>> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("playback session lock poisoned"))
    }

    /// Local VoiceSession Ready hook. Never call this from playback RPCs.
    pub fn activate(&self, scope: &VoiceScope) -> Result<()> {
        validate_scope(scope)?;
        let mut state = self.lock()?;
        if let Some(active) = state.as_ref() {
            if same_session(&active.scope, scope) {
                ensure!(
                    scope.generation_epoch >= active.scope.generation_epoch,
                    "Ready epoch regressed"
                );
                drop(state);
                self.fence(scope, false)?;
                return Ok(());
            }
        }
        let old = state.take();
        if let Some(generation) = old.and_then(|active| active.generation) {
            self.backend.stop_if_current(generation)?;
        }
        *state = Some(ActiveSession {
            scope: scope.clone(),
            highest_sequence: None,
            generation: None,
            current: None,
            input_ended: true,
        });
        Ok(())
    }

    /// Local stream-close hook. A late close from an old stream has no effect.
    pub fn close(&self, scope: &VoiceScope) -> Result<bool> {
        let mut state = self.lock()?;
        let Some(active) = state.as_ref() else {
            return Ok(false);
        };
        if !same_session(&active.scope, scope) {
            return Ok(false);
        }
        let generation = active.generation;
        *state = None;
        if let Some(generation) = generation {
            self.backend.stop_if_current(generation)?;
        }
        Ok(true)
    }

    pub fn fence(&self, minimum: &VoiceScope, close_session: bool) -> Result<bool> {
        validate_scope(minimum)?;
        let mut state = self.lock()?;
        let Some(active) = state.as_mut() else {
            return Ok(false);
        };
        if !same_session(&active.scope, minimum) {
            return Ok(false);
        }
        if close_session {
            let generation = active.generation;
            *state = None;
            if let Some(generation) = generation {
                self.backend.stop_if_current(generation)?;
            }
            return Ok(true);
        }
        if minimum.generation_epoch <= active.scope.generation_epoch {
            return Ok(false);
        }
        let generation = active.generation;
        active.scope.generation_epoch = minimum.generation_epoch;
        active.generation = None;
        active.current = None;
        active.input_ended = true;
        if let Some(generation) = generation {
            self.backend.stop_if_current(generation)?;
        }
        Ok(true)
    }

    pub fn begin(
        &self,
        scope: &VoiceScope,
        playback_id: &str,
        playback_sequence: u64,
        sample_rate: u32,
    ) -> Result<PlaybackLease> {
        validate_scope(scope)?;
        ensure!(
            !Uuid::parse_str(playback_id)?.is_nil(),
            "playback id must be a non-nil UUID"
        );
        ensure!(
            (8000..=192000).contains(&sample_rate),
            "playback sample rate outside 8–192 kHz"
        );
        let mut state = self.lock()?;
        let active = state
            .as_mut()
            .context("no locally activated voice session")?;
        ensure!(
            same_session(&active.scope, scope),
            "playback session identity is stale"
        );
        ensure!(
            scope.generation_epoch >= active.scope.generation_epoch,
            "playback epoch is stale"
        );
        ensure!(
            active
                .highest_sequence
                .is_none_or(|previous| playback_sequence > previous),
            "playback sequence was already used or is stale"
        );
        let newer_epoch = scope.generation_epoch > active.scope.generation_epoch;
        ensure!(
            newer_epoch || active.input_ended,
            "previous playback Push is still receiving media"
        );
        let previous_generation = active.generation;
        active.scope = *scope;
        active.highest_sequence = Some(playback_sequence);
        active.current = None;
        active.generation = None;
        active.input_ended = true;
        let generation = match previous_generation {
            Some(generation) if !newer_epoch && self.backend.is_playback_current(generation) => {
                generation
            }
            _ => self.backend.begin_scoped_playback()?,
        };
        let lease = PlaybackLease {
            scope: scope.clone(),
            playback_id: playback_id.into(),
            playback_sequence,
            sample_rate,
            generation,
        };
        active.scope = scope.clone();
        active.highest_sequence = Some(playback_sequence);
        active.generation = Some(generation);
        active.current = Some(lease.clone());
        active.input_ended = false;
        Ok(lease)
    }

    /// The state lock is released before waiting for media capacity. The backend
    /// validates the captured generation atomically with its own queue insertion.
    pub fn enqueue(&self, lease: &PlaybackLease, pcm: &[i16]) -> Result<bool> {
        {
            let state = self.lock()?;
            let Some(active) = state.as_ref() else {
                return Ok(false);
            };
            if !matches_lease(active, lease) || active.input_ended {
                return Ok(false);
            }
        }
        self.backend
            .play_if_current(pcm, lease.sample_rate, lease.generation)
    }

    pub fn end(&self, lease: &PlaybackLease) -> Result<bool> {
        let mut state = self.lock()?;
        let Some(active) = state.as_mut() else {
            return Ok(false);
        };
        if !matches_lease(active, lease)
            || active.input_ended
            || !self.backend.is_playback_current(lease.generation)
        {
            return Ok(false);
        }
        active.input_ended = true;
        Ok(true)
    }

    pub fn is_current(&self, lease: &PlaybackLease) -> Result<bool> {
        Ok(self
            .lock()?
            .as_ref()
            .is_some_and(|active| matches_lease(active, lease))
            && self.backend.is_playback_current(lease.generation))
    }

    pub fn cancel(&self, lease: &PlaybackLease) -> Result<bool> {
        let mut state = self.lock()?;
        let Some(active) = state.as_mut() else {
            return Ok(false);
        };
        if !matches_lease(active, lease) {
            return Ok(false);
        }
        active.generation = None;
        active.current = None;
        active.input_ended = true;
        let stopped = self.backend.stop_if_current(lease.generation)?;
        Ok(stopped)
    }

    pub fn is_active(&self) -> bool {
        self.state.lock().is_ok_and(|state| state.is_some())
    }

    pub fn active_scope(&self) -> Result<Option<VoiceScope>> {
        Ok(self.lock()?.as_ref().map(|active| active.scope))
    }
}

fn matches_lease(active: &ActiveSession, lease: &PlaybackLease) -> bool {
    active.current.as_ref().is_some_and(|current| {
        current.scope == lease.scope
            && current.playback_id == lease.playback_id
            && current.playback_sequence == lease.playback_sequence
            && current.generation == lease.generation
    })
}

fn same_session(left: &VoiceScope, right: &VoiceScope) -> bool {
    left.device_boot_id == right.device_boot_id && left.session_id == right.session_id
}

fn validate_scope(scope: &VoiceScope) -> Result<()> {
    ensure!(scope.is_valid(), "invalid playback voice scope");
    Ok(())
}

#[cfg(test)]
mod tests;
