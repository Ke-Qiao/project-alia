use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::channels::adapter::ChannelEvent;

/// Internal event types
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "payload")]
pub enum ServerEvent {
    // Channel pipeline events
    ChannelInput(ChannelEvent),

    // Device events
    DeviceOnline {
        device_id: String,
    },
    DeviceOffline {
        device_id: String,
        reason: String,
    },
    DeviceDiscovered {
        channel_id: String,
        endpoint: String,
        device_id: Option<String>,
        device_name: Option<String>,
        status: String,
        provisioning_mode: bool,
        bind_required: bool,
    },

    // Plugin events
    PluginEvent {
        plugin_name: String,
        event_type: String,
        data: serde_json::Value,
    },

    // Voice events
    VoiceStateChanged {
        state: String,
    },

    // Rule events
    RuleTriggered {
        rule_id: Uuid,
        rule_name: String,
        action: String,
    },

    // Insights events
    InsightStarted {
        phase: String,
        trigger: String,
    },
    InsightCompleted {
        phase: String,
        nodes_created: u32,
        nodes_updated: u32,
    },

    // Scheduler events
    TaskScheduled {
        task_id: Uuid,
        task_name: String,
        next_run: DateTime<Utc>,
    },
    TaskTriggered {
        task_id: Uuid,
        task_name: String,
        trigger_source: String,
    },
    TaskCompleted {
        task_id: Uuid,
        task_name: String,
        success: bool,
        duration_ms: u64,
    },
    TaskFailed {
        task_id: Uuid,
        task_name: String,
        error: String,
        retry_count: u32,
    },

    // Tool events
    ToolCallStarted {
        call_id: String,
        tool_name: String,
        source: String,
    },
    ToolCallCompleted {
        call_id: String,
        tool_name: String,
        success: bool,
        duration_ms: u64,
    },
    ToolConfirmationRequired {
        call_id: String,
        tool_name: String,
        description: String,
        timeout_sec: u32,
    },

    // Sensor events
    SensorDataIngested {
        sensor_id: String,
        source: String,
        value: serde_json::Value,
    },

    // Streaming LLM events
    StreamChunk {
        channel_id: String,
        user_id: String,
        chunk: String,
    },
    StreamEnd {
        channel_id: String,
        user_id: String,
    },

    // Settings events
    SettingsChanged {
        keys: Vec<String>,
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
