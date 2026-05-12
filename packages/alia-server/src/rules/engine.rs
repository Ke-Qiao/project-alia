//! Simple Rule Engine
//!
//! A basic rule engine that evaluates automation rules against events.
//! Rules consist of:
//! - Trigger: Event type that activates the rule
//! - Conditions: Optional filters on event data
//! - Actions: What to do when rule matches

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::event::ServerEvent;
use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{RuleAction, RuleEvaluator};

/// Rule condition operators
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Condition {
    #[serde(rename = "eq")]
    Equals {
        field: String,
        value: serde_json::Value,
    },
    #[serde(rename = "neq")]
    NotEquals {
        field: String,
        value: serde_json::Value,
    },
    #[serde(rename = "contains")]
    Contains { field: String, value: String },
    #[serde(rename = "gt")]
    GreaterThan { field: String, value: f64 },
    #[serde(rename = "lt")]
    LessThan { field: String, value: f64 },
    #[serde(rename = "and")]
    And { conditions: Vec<Condition> },
    #[serde(rename = "or")]
    Or { conditions: Vec<Condition> },
}

/// Automation rule definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub trigger_event: String,
    pub conditions: Option<Condition>,
    pub action_type: String,
    pub action_payload: serde_json::Value,
}

/// Simple rule engine
pub struct SimpleRuleEngine {
    pub rules: Arc<RwLock<Vec<Rule>>>,
}

impl SimpleRuleEngine {
    pub fn new() -> Self {
        Self {
            rules: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Extract event type name for matching
    fn event_type_name(event: &ServerEvent) -> &'static str {
        match event {
            ServerEvent::DeviceOnline { .. } => "device_online",
            ServerEvent::DeviceOffline { .. } => "device_offline",
            ServerEvent::DeviceDiscovered { .. } => "device_discovered",
            ServerEvent::PluginEvent { .. } => "plugin_event",
            ServerEvent::ProcessOutput { .. } => "process_output",
            ServerEvent::ProcessStateChanged { .. } => "process_state_changed",
            ServerEvent::VoiceStateChanged { .. } => "voice_state_changed",
            ServerEvent::RuleTriggered { .. } => "rule_triggered",
            ServerEvent::MeditationStarted { .. } => "meditation_started",
            ServerEvent::MeditationCompleted { .. } => "meditation_completed",
            ServerEvent::TaskScheduled { .. } => "task_scheduled",
            ServerEvent::TaskTriggered { .. } => "task_triggered",
            ServerEvent::TaskCompleted { .. } => "task_completed",
            ServerEvent::TaskFailed { .. } => "task_failed",
            ServerEvent::ToolCallStarted { .. } => "tool_call_started",
            ServerEvent::ToolCallCompleted { .. } => "tool_call_completed",
            ServerEvent::ToolConfirmationRequired { .. } => "tool_confirmation_required",
            ServerEvent::ChannelInput(_) => "channel_input",
            ServerEvent::SensorDataIngested { .. } => "sensor_data_ingested",
            ServerEvent::StreamChunk { .. } => "stream_chunk",
            ServerEvent::StreamEnd { .. } => "stream_end",
            ServerEvent::SettingsChanged { .. } => "settings_changed",
        }
    }

    /// Convert event to JSON for condition evaluation
    fn event_to_json(event: &ServerEvent) -> serde_json::Value {
        serde_json::to_value(event).unwrap_or(serde_json::Value::Null)
    }

    /// Evaluate a condition against event data
    pub fn evaluate_condition(condition: &Condition, data: &serde_json::Value) -> bool {
        match condition {
            Condition::Equals { field, value } => {
                let field_value = Self::get_field(data, field);
                &field_value == value
            }
            Condition::NotEquals { field, value } => {
                let field_value = Self::get_field(data, field);
                &field_value != value
            }
            Condition::Contains { field, value } => {
                let field_value = Self::get_field(data, field);
                if let serde_json::Value::String(s) = field_value {
                    s.contains(value)
                } else {
                    false
                }
            }
            Condition::GreaterThan { field, value } => {
                let field_value = Self::get_field(data, field);
                if let serde_json::Value::Number(n) = field_value {
                    n.as_f64().map(|v| v > *value).unwrap_or(false)
                } else {
                    false
                }
            }
            Condition::LessThan { field, value } => {
                let field_value = Self::get_field(data, field);
                if let serde_json::Value::Number(n) = field_value {
                    n.as_f64().map(|v| v < *value).unwrap_or(false)
                } else {
                    false
                }
            }
            Condition::And { conditions } => {
                conditions.iter().all(|c| Self::evaluate_condition(c, data))
            }
            Condition::Or { conditions } => {
                conditions.iter().any(|c| Self::evaluate_condition(c, data))
            }
        }
    }

    /// Get nested field value using dot notation (e.g., "payload.device_id")
    fn get_field(data: &serde_json::Value, path: &str) -> serde_json::Value {
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = data;

        for part in parts {
            match current {
                serde_json::Value::Object(map) => {
                    current = map.get(part).unwrap_or(&serde_json::Value::Null);
                }
                _ => return serde_json::Value::Null,
            }
        }

        current.clone()
    }
}

impl Default for SimpleRuleEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModuleLifecycle for SimpleRuleEngine {
    fn name(&self) -> &'static str {
        "simple"
    }

    async fn init(&mut self, _ctx: &ModuleContext) -> anyhow::Result<()> {
        // TODO: Load rules from database
        // For now, start with empty rules
        tracing::info!("SimpleRuleEngine initialized");
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        ModuleHealth::Healthy
    }
}

#[async_trait]
impl RuleEvaluator for SimpleRuleEngine {
    async fn evaluate(&self, event: &ServerEvent) -> anyhow::Result<Vec<RuleAction>> {
        let rules = self.rules.read().await;
        let event_type = Self::event_type_name(event);
        let event_data = Self::event_to_json(event);

        let mut actions = Vec::new();

        for rule in rules.iter() {
            if !rule.enabled {
                continue;
            }

            // Check if event type matches trigger
            if rule.trigger_event != event_type && rule.trigger_event != "*" {
                continue;
            }

            // Evaluate conditions if present
            let conditions_match = match &rule.conditions {
                Some(cond) => Self::evaluate_condition(cond, &event_data),
                None => true,
            };

            if conditions_match {
                tracing::debug!(
                    rule_id = %rule.id,
                    rule_name = %rule.name,
                    "rule matched"
                );

                actions.push(RuleAction {
                    rule_id: rule.id,
                    rule_name: rule.name.clone(),
                    action_type: rule.action_type.clone(),
                    payload: rule.action_payload.clone(),
                });
            }
        }

        Ok(actions)
    }

    async fn reload(&self) -> anyhow::Result<()> {
        // TODO: Reload rules from database
        tracing::info!("Rule engine reload requested");
        Ok(())
    }
}
