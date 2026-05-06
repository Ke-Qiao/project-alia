use serde::Serialize;
use tokio::sync::broadcast;

/// Internal event types
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "payload")]
pub enum ServerEvent {
    DeviceOnline {
        device_id: String,
    },
    DeviceOffline {
        device_id: String,
        reason: String,
    },
    PluginEvent {
        plugin_name: String,
        event_type: String,
        data: serde_json::Value,
    },
    ProcessOutput {
        name: String,
        channel: String,
        line: String,
    },
    ProcessStateChanged {
        name: String,
        old_state: String,
        new_state: String,
    },
    VoiceStateChanged {
        state: String,
    },
    RuleTriggered {
        rule_id: uuid::Uuid,
        rule_name: String,
        action: String,
    },
    MeditationStarted {
        phase: String,
        trigger: String,
    },
    MeditationCompleted {
        phase: String,
        nodes_created: u32,
        nodes_updated: u32,
    },
}

/// Event bus wrapping tokio broadcast channel
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<ServerEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn publish(&self, event: ServerEvent) {
        // No subscribers is normal, ignore the error
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.sender.subscribe()
    }
}
