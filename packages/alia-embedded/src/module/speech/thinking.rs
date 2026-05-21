use crate::config::ThinkingConfig;

#[derive(Debug, Clone)]
pub(super) struct ServerTurnState {
    config: ThinkingConfig,
    mode: ServerTurnMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerTurnMode {
    Idle,
    Waiting {
        next_cue_at_us: u64,
        cue_missing_logged: bool,
        cue_disabled: bool,
    },
}

impl ServerTurnState {
    pub(super) fn new(config: ThinkingConfig) -> Self {
        Self {
            config,
            mode: ServerTurnMode::Idle,
        }
    }

    pub(super) fn action(&self) -> &str {
        self.config.action.as_str()
    }

    pub(super) fn mark_waiting(&mut self, timestamp_us: u64) {
        self.mode = ServerTurnMode::Waiting {
            next_cue_at_us: timestamp_us.saturating_add(ms_to_us(self.config.initial_delay_ms)),
            cue_missing_logged: false,
            cue_disabled: false,
        };
    }

    pub(super) fn is_waiting(&self) -> bool {
        matches!(self.mode, ServerTurnMode::Waiting { .. })
    }

    pub(super) fn cue_due(&self, timestamp_us: u64, playback_active: bool) -> bool {
        if playback_active || !self.config.enabled || self.config.action.trim().is_empty() {
            return false;
        }
        match self.mode {
            ServerTurnMode::Waiting {
                next_cue_at_us,
                cue_disabled,
                ..
            } => !cue_disabled && timestamp_us >= next_cue_at_us,
            ServerTurnMode::Idle => false,
        }
    }

    pub(super) fn mark_cue_played(&mut self, timestamp_us: u64) {
        let ServerTurnMode::Waiting {
            next_cue_at_us,
            cue_disabled,
            ..
        } = &mut self.mode
        else {
            return;
        };

        if self.config.repeat_interval_ms == 0 {
            *cue_disabled = true;
        } else {
            *next_cue_at_us =
                timestamp_us.saturating_add(ms_to_us(self.config.repeat_interval_ms.max(250)));
        }
    }

    pub(super) fn mark_cue_missing(&mut self) -> bool {
        let ServerTurnMode::Waiting {
            cue_missing_logged,
            cue_disabled,
            ..
        } = &mut self.mode
        else {
            return false;
        };
        *cue_disabled = true;
        if *cue_missing_logged {
            return false;
        }
        *cue_missing_logged = true;
        true
    }

    pub(super) fn stop(&mut self) -> bool {
        let was_waiting = self.is_waiting();
        self.mode = ServerTurnMode::Idle;
        was_waiting
    }
}

fn ms_to_us(ms: u32) -> u64 {
    u64::from(ms).saturating_mul(1_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ThinkingConfig {
        ThinkingConfig {
            enabled: true,
            action: "thinking".to_string(),
            initial_delay_ms: 1_200,
            repeat_interval_ms: 3_000,
        }
    }

    #[test]
    fn waits_before_first_cue_then_repeats_on_interval() {
        let mut state = ServerTurnState::new(config());
        state.mark_waiting(10_000);

        assert!(!state.cue_due(1_209_999, false));
        assert!(state.cue_due(1_210_000, false));

        state.mark_cue_played(1_210_000);
        assert!(!state.cue_due(4_209_999, false));
        assert!(state.cue_due(4_210_000, false));
    }

    #[test]
    fn assistant_start_or_interrupt_stops_waiting_state() {
        let mut state = ServerTurnState::new(config());
        state.mark_waiting(0);

        assert!(state.is_waiting());
        assert!(state.stop());
        assert!(!state.is_waiting());
        assert!(!state.cue_due(10_000_000, false));
    }

    #[test]
    fn missing_cue_logs_once_and_disables_current_wait() {
        let mut state = ServerTurnState::new(config());
        state.mark_waiting(0);

        assert!(state.mark_cue_missing());
        assert!(!state.mark_cue_missing());
        assert!(!state.cue_due(10_000_000, false));
    }

    #[test]
    fn playback_suppresses_cue_without_ending_wait() {
        let mut state = ServerTurnState::new(config());
        state.mark_waiting(0);

        assert!(!state.cue_due(2_000_000, true));
        assert!(state.is_waiting());
    }
}
