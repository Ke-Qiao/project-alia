use crate::config::ContinuousDialogueConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContinuousDialogueMode {
    Idle,
    WaitingForAssistant,
    FollowupListening { deadline_us: u64 },
}

#[derive(Debug, Clone)]
pub(super) struct ContinuousDialogueState {
    enabled: bool,
    followup_timeout_ms: u32,
    max_followup_turns: u32,
    session_active: bool,
    followup_turns: u32,
    start_followup_next_frame: bool,
    mode: ContinuousDialogueMode,
}

impl ContinuousDialogueState {
    pub(super) fn new(config: ContinuousDialogueConfig) -> Self {
        Self {
            enabled: config.enabled,
            followup_timeout_ms: config.followup_timeout_ms,
            max_followup_turns: config.max_followup_turns,
            session_active: false,
            followup_turns: 0,
            start_followup_next_frame: false,
            mode: ContinuousDialogueMode::Idle,
        }
    }

    pub(super) fn start_session(&mut self) {
        if !self.enabled {
            return;
        }
        self.session_active = true;
        self.followup_turns = 0;
        self.start_followup_next_frame = false;
        self.mode = ContinuousDialogueMode::Idle;
    }

    pub(super) fn mark_waiting_for_assistant(&mut self) {
        if self.enabled && self.session_active {
            self.start_followup_next_frame = false;
            self.mode = ContinuousDialogueMode::WaitingForAssistant;
        }
    }

    pub(super) fn mark_assistant_speaking(&mut self) {
        if self.enabled && self.session_active {
            self.start_followup_next_frame = false;
            self.mode = ContinuousDialogueMode::WaitingForAssistant;
        }
    }

    pub(super) fn schedule_followup(&mut self) {
        if !self.enabled || !self.session_active {
            return;
        }
        if !self.can_start_followup() {
            self.end_session();
            return;
        }
        self.mode = ContinuousDialogueMode::Idle;
        self.start_followup_next_frame = true;
    }

    pub(super) fn start_followup_if_scheduled(&mut self, timestamp_us: u64) -> bool {
        if !self.start_followup_next_frame || !self.can_start_followup() {
            return false;
        }
        self.start_followup_next_frame = false;
        let timeout_us = u64::from(self.followup_timeout_ms).saturating_mul(1_000);
        self.mode = ContinuousDialogueMode::FollowupListening {
            deadline_us: timestamp_us.saturating_add(timeout_us),
        };
        true
    }

    pub(super) fn mark_followup_capture_started(&mut self) {
        if !matches!(self.mode, ContinuousDialogueMode::FollowupListening { .. }) {
            return;
        }
        self.followup_turns = self.followup_turns.saturating_add(1);
        self.mode = ContinuousDialogueMode::Idle;
        self.start_followup_next_frame = false;
    }

    pub(super) fn expire_if_needed(&mut self, timestamp_us: u64) -> bool {
        let ContinuousDialogueMode::FollowupListening { deadline_us } = self.mode else {
            return false;
        };
        if timestamp_us < deadline_us {
            return false;
        }
        self.end_session();
        true
    }

    pub(super) fn end_session(&mut self) {
        self.session_active = false;
        self.followup_turns = 0;
        self.start_followup_next_frame = false;
        self.mode = ContinuousDialogueMode::Idle;
    }

    pub(super) fn is_followup_listening(&self) -> bool {
        matches!(self.mode, ContinuousDialogueMode::FollowupListening { .. })
    }

    fn can_start_followup(&self) -> bool {
        self.max_followup_turns == 0 || self.followup_turns < self.max_followup_turns
    }

    pub(super) fn followup_timeout_ms(&self) -> u32 {
        self.followup_timeout_ms
    }

    pub(super) fn max_followup_turns(&self) -> u32 {
        self.max_followup_turns
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_dialogue_starts_followup_after_assistant_reply() {
        let mut state = ContinuousDialogueState::new(ContinuousDialogueConfig {
            enabled: true,
            followup_timeout_ms: 10_000,
            max_followup_turns: 6,
        });

        state.start_session();
        state.mark_waiting_for_assistant();
        state.schedule_followup();

        assert!(state.start_followup_if_scheduled(1_000));
        assert!(state.is_followup_listening());
        assert!(!state.expire_if_needed(1_000 + 9_999_000));
        assert!(state.expire_if_needed(1_000 + 10_000_000));
        assert!(!state.is_followup_listening());
    }

    #[test]
    fn continuous_dialogue_counts_followup_turns() {
        let mut state = ContinuousDialogueState::new(ContinuousDialogueConfig {
            enabled: true,
            followup_timeout_ms: 10_000,
            max_followup_turns: 1,
        });

        state.start_session();
        state.schedule_followup();
        assert!(state.start_followup_if_scheduled(1_000));
        state.mark_followup_capture_started();
        state.schedule_followup();

        assert!(!state.start_followup_if_scheduled(2_000));
        assert!(!state.is_followup_listening());
    }
}
