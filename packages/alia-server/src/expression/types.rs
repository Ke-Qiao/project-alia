use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::providers::adapter::{MemoryContextPack, TtsCapabilities};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AffectAxes {
    pub valence: f32,
    pub arousal: f32,
    pub calmness: f32,
}

impl Default for AffectAxes {
    fn default() -> Self {
        Self {
            valence: 0.05,
            arousal: 0.35,
            calmness: 0.65,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SocialState {
    pub trust: f32,
    pub familiarity: f32,
    pub engagement: f32,
}

impl Default for SocialState {
    fn default() -> Self {
        Self {
            trust: 0.55,
            familiarity: 0.15,
            engagement: 0.55,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ResourceState {
    pub fatigue: f32,
    pub battery_stress: f32,
    pub thermal_stress: f32,
    pub sleepiness: f32,
    pub interaction_load: f32,
}

impl Default for ResourceState {
    fn default() -> Self {
        Self {
            fatigue: 0.05,
            battery_stress: 0.0,
            thermal_stress: 0.0,
            sleepiness: 0.05,
            interaction_load: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffectSnapshot {
    pub affect: AffectAxes,
    pub social: SocialState,
    pub resource: ResourceState,
    pub dominant_causes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipProfile {
    #[serde(default)]
    pub relationship_epoch: i64,
    pub user_id: String,
    pub trust: f32,
    pub familiarity: f32,
    pub summary: String,
    #[serde(default)]
    pub preference_summary: String,
    #[serde(default)]
    pub preferences: BTreeMap<String, String>,
    #[serde(default)]
    pub signal_counts: BTreeMap<String, usize>,
    pub privacy: RelationshipPrivacyControls,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipPrivacyControls {
    pub user_visible: bool,
    pub stores_preferences: bool,
    pub stores_raw_evidence: bool,
    pub clearable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipClearResult {
    pub user_id: String,
    pub profile_removed: bool,
    pub relationship_events_removed: usize,
    pub persisted_events_removed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipPruneResult {
    pub profiles_removed: usize,
    pub relationship_states_removed: usize,
}

impl Default for AffectSnapshot {
    fn default() -> Self {
        Self {
            affect: AffectAxes::default(),
            social: SocialState::default(),
            resource: ResourceState::default(),
            dominant_causes: vec!["baseline".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionInput {
    pub response_text: String,
    pub route: Option<String>,
    pub channel_id: String,
    pub user_id: String,
    #[serde(default = "default_dialogue_act")]
    pub dialogue_act: String,
    #[serde(default = "default_content_tone")]
    pub content_tone: String,
    #[serde(default)]
    pub user_state: ExpressionUserState,
    #[serde(default)]
    pub device_context: ExpressionDeviceContext,
    #[serde(default)]
    pub safety_context: ExpressionSafetyContext,
    #[serde(default)]
    pub conversation_context: ExpressionConversationContext,
    #[serde(default)]
    pub constraints: ExpressionConstraints,
    #[serde(default)]
    pub memory_context: Option<MemoryContextPack>,
    pub voice_output: bool,
    pub hardware_control: bool,
    #[serde(default)]
    pub tts_capabilities: Option<TtsCapabilities>,
    #[serde(default)]
    pub expression_capabilities: Option<ExpressionCapabilitySnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionConversationContext {
    #[serde(default)]
    pub turn_kind: String,
    #[serde(default)]
    pub route: Option<String>,
    #[serde(default)]
    pub used_tools: bool,
    #[serde(default)]
    pub response_was_fallback: bool,
    #[serde(default)]
    pub cache_hit: bool,
    #[serde(default)]
    pub prior_message_count: usize,
}

impl Default for ExpressionConversationContext {
    fn default() -> Self {
        Self {
            turn_kind: "assistant_response".to_string(),
            route: None,
            used_tools: false,
            response_was_fallback: false,
            cache_hit: false,
            prior_message_count: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExpressionConstraints {
    #[serde(default)]
    pub prefer_minimal: bool,
    #[serde(default)]
    pub require_clarity: bool,
    #[serde(default)]
    pub allow_playful: bool,
    #[serde(default)]
    pub max_intensity: Option<f32>,
    #[serde(default)]
    pub max_motion_intensity: Option<f32>,
}

impl ExpressionInput {
    pub fn effective_device_context(&self) -> ExpressionDeviceContext {
        let mut context = self.device_context.clone();
        if let Some(capabilities) = &self.expression_capabilities {
            context.supports_motion |= capabilities.current_state.motion_adapter_registered
                || capabilities.capabilities.iter().any(|(name, capability)| {
                    capability.supported
                        && (name.starts_with("motion.")
                            || matches!(
                                name.as_str(),
                                "look_at_user" | "small_nod" | "small_bow" | "shake_head"
                            ))
                });
            context.thermal_limit |= capabilities.current_state.thermal_limit;
            context.battery_saver |= capabilities.current_state.battery_saver;
        }
        context
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionUserState {
    pub frustration: f32,
    pub engagement: f32,
}

impl Default for ExpressionUserState {
    fn default() -> Self {
        Self {
            frustration: 0.0,
            engagement: 0.55,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionDeviceContext {
    #[serde(default)]
    pub supports_motion: bool,
    #[serde(default)]
    pub supports_face: bool,
    #[serde(default)]
    pub supports_light: bool,
    #[serde(default)]
    pub supports_haptics: bool,
    #[serde(default)]
    pub battery_level: Option<f32>,
    #[serde(default)]
    pub battery_saver: bool,
    #[serde(default)]
    pub thermal_limit: bool,
}

impl Default for ExpressionDeviceContext {
    fn default() -> Self {
        Self {
            supports_motion: false,
            supports_face: false,
            supports_light: false,
            supports_haptics: false,
            battery_level: None,
            battery_saver: false,
            thermal_limit: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionSafetyContext {
    #[serde(default = "default_true")]
    pub motion_allowed: bool,
    #[serde(default)]
    pub active_task_serious: bool,
    #[serde(default)]
    pub minimal_expression: bool,
    #[serde(default)]
    pub night_mode: bool,
    #[serde(default)]
    pub volume_limit: Option<f32>,
    #[serde(default)]
    pub max_expression_intensity: Option<f32>,
    #[serde(default)]
    pub max_motion_intensity: Option<f32>,
}

impl Default for ExpressionSafetyContext {
    fn default() -> Self {
        Self {
            motion_allowed: true,
            active_task_serious: false,
            minimal_expression: false,
            night_mode: false,
            volume_limit: None,
            max_expression_intensity: None,
            max_motion_intensity: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionIntent {
    pub intent: String,
    pub social_function: String,
    pub style: String,
    pub intensity: f32,
    pub priority: ExpressionPriority,
    pub interruptible: bool,
    pub safety_class: ExpressionSafetyClass,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionPlan {
    pub id: Uuid,
    pub intent: String,
    pub social_function: String,
    pub style: String,
    pub intensity: f32,
    pub stance: ExpressiveStance,
    pub priority: ExpressionPriority,
    pub interruptible: bool,
    pub safety_class: ExpressionSafetyClass,
    pub speech: Option<SpeechExpression>,
    pub timeline: Vec<ExpressionTimelineAction>,
    #[serde(default)]
    pub safety_modifications: Vec<ExpressionSafetyModification>,
    pub fallback: FallbackPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressiveStance {
    pub strategy: String,
    pub style_hint: Option<String>,
    pub expressed_valence: f32,
    pub expressed_arousal: f32,
    pub warmth: f32,
    pub intensity_scale: f32,
    pub max_intensity: f32,
    pub motion_scale: f32,
    pub rate_offset: f32,
    pub energy_offset: f32,
    pub pause_bias_offset: f32,
    pub reasons: Vec<String>,
}

impl Default for ExpressiveStance {
    fn default() -> Self {
        Self {
            strategy: "neutral".to_string(),
            style_hint: None,
            expressed_valence: 0.05,
            expressed_arousal: 0.35,
            warmth: 0.55,
            intensity_scale: 1.0,
            max_intensity: 0.7,
            motion_scale: 1.0,
            rate_offset: 0.0,
            energy_offset: 0.0,
            pause_bias_offset: 0.0,
            reasons: vec!["baseline".to_string()],
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExpressionPriority {
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExpressionSafetyClass {
    LowRisk,
    MediumRisk,
    HighRisk,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionSafetyModification {
    pub policy: String,
    pub path: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechExpression {
    pub style: String,
    pub intensity: f32,
    pub voice_profile: Option<String>,
    pub prosody: ProsodyControls,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ProsodyControls {
    pub rate: f32,
    pub pitch_shift: f32,
    pub energy: f32,
    pub pause_bias: f32,
}

impl Default for ProsodyControls {
    fn default() -> Self {
        Self {
            rate: 1.0,
            pitch_shift: 0.0,
            energy: 0.5,
            pause_bias: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionTimelineAction {
    pub at: String,
    pub channel: String,
    pub action: String,
    pub intensity: f32,
    pub duration_ms: u32,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackPolicy {
    pub motion_unavailable: String,
    pub face_unavailable: String,
}

impl Default for FallbackPolicy {
    fn default() -> Self {
        Self {
            motion_unavailable: "speech_only".to_string(),
            face_unavailable: "speech_only".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionStyle {
    pub name: String,
    pub voice_profile: Option<String>,
    pub prosody: ProsodyControls,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpressionExecutionResult {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    #[serde(default)]
    pub voice_scope: Option<alia_core::delivery::VoiceScope>,
    /// Original command and raw device facts; local observation failure is kept
    /// separate and never stamped into the device's monotonic receipt stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyExecutionEvidence>,
    #[serde(default)]
    pub action_receipts: Vec<ExpressionActionReceipt>,
    #[serde(default)]
    pub skipped_actions: Vec<SkippedExpressionAction>,
    #[serde(default)]
    pub safety_modifications: Vec<serde_json::Value>,
}

impl ExpressionExecutionResult {
    pub fn skipped(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            status: "skipped".to_string(),
            command_id: None,
            voice_scope: None,
            body: None,
            action_receipts: vec![ExpressionActionReceipt {
                action: "*".to_string(),
                status: ExpressionActionStatus::Skipped,
                reason: Some(reason.clone()),
            }],
            skipped_actions: vec![SkippedExpressionAction {
                action: "*".to_string(),
                reason,
            }],
            safety_modifications: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyExecutionEvidence {
    pub command: alia_core::body::BodyCommand,
    pub progress: Option<alia_core::body::BodyProgress>,
    pub unknown_reason: Option<String>,
}

impl BodyExecutionEvidence {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.command.validate()?;
        if let Some(progress) = &self.progress {
            progress.validate(None, &self.command)?;
        }
        Ok(())
    }

    pub fn writer_retired(&self) -> bool {
        self.progress
            .as_ref()
            .is_some_and(|progress| progress.writer_quiescent && progress.end.is_some())
    }
}

impl ExpressionExecutionResult {
    /// The v1 body driver is open loop. Submitted and Cancelled can retire a
    /// software writer, but neither is evidence that a motor reached a position.
    pub(crate) fn from_body(
        body: BodyExecutionEvidence,
        actions: &[String],
        cancellation: bool,
    ) -> Self {
        use alia_core::body::BodyEnd;
        let progress = body.progress.as_ref();
        let end = progress.and_then(|progress| progress.end.as_ref());
        let local_unknown = body.unknown_reason.is_some();
        let (status, terminal_action, reason) = if local_unknown {
            (
                "unknown",
                ExpressionActionStatus::Unknown,
                body.unknown_reason.clone(),
            )
        } else {
            match end {
                Some(BodyEnd::Submitted) => ("submitted", ExpressionActionStatus::Submitted, None),
                Some(BodyEnd::Cancelled { reason }) => (
                    "cancelled",
                    ExpressionActionStatus::Cancelled,
                    Some(reason.clone()),
                ),
                Some(BodyEnd::Failed { reason }) => (
                    "failed",
                    ExpressionActionStatus::Failed,
                    Some(reason.clone()),
                ),
                Some(BodyEnd::Rejected { reason }) => (
                    "rejected",
                    ExpressionActionStatus::Rejected,
                    Some(reason.clone()),
                ),
                Some(BodyEnd::Unknown { reason }) => (
                    "unknown",
                    ExpressionActionStatus::Unknown,
                    Some(reason.clone()),
                ),
                None if progress.is_some_and(|progress| progress.started_actions > 0) => {
                    ("started", ExpressionActionStatus::Started, None)
                }
                None if progress.is_some() => ("accepted", ExpressionActionStatus::Accepted, None),
                None => (
                    "unknown",
                    ExpressionActionStatus::Unknown,
                    Some("body command admission has no acknowledgement".into()),
                ),
            }
        };
        let action_receipts = actions
            .iter()
            .enumerate()
            .map(|(index, action)| {
                let observed = if cancellation {
                    terminal_action.clone()
                } else if progress.is_some_and(|p| index < p.submitted_actions as usize) {
                    ExpressionActionStatus::Submitted
                } else if end.is_some() || local_unknown {
                    terminal_action.clone()
                } else if progress.is_some_and(|p| index < p.started_actions as usize) {
                    ExpressionActionStatus::Started
                } else if progress.is_some_and(|p| index < p.accepted_actions as usize) {
                    ExpressionActionStatus::Accepted
                } else {
                    ExpressionActionStatus::Unknown
                };
                ExpressionActionReceipt {
                    action: action.clone(),
                    status: observed,
                    reason: reason.clone(),
                }
            })
            .collect();
        Self {
            status: status.into(),
            command_id: Some(body.command.source_command_id.clone()),
            voice_scope: body.command.scope.voice(),
            body: Some(body),
            action_receipts,
            skipped_actions: Vec::new(),
            safety_modifications: Vec::new(),
        }
    }

    pub(crate) fn writer_retired(&self) -> bool {
        self.body
            .as_ref()
            .is_some_and(BodyExecutionEvidence::writer_retired)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExpressionActionStatus {
    Accepted,
    Started,
    Submitted,
    Cancelled,
    Failed,
    Unknown,
    Rejected,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionActionReceipt {
    pub action: String,
    pub status: ExpressionActionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedExpressionAction {
    pub action: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionCapabilitySnapshot {
    pub adapter: String,
    /// Opt-in contract for command-bound execution and exact cancellation.
    /// Advertising actions alone is not proof of this protocol.
    #[serde(default)]
    pub command_protocol: Option<String>,
    #[serde(default)]
    pub capabilities: BTreeMap<String, ExpressionActionCapability>,
    #[serde(default)]
    pub current_state: ExpressionAdapterState,
}

impl ExpressionCapabilitySnapshot {
    pub fn unsupported(adapter: impl Into<String>) -> Self {
        Self {
            adapter: adapter.into(),
            command_protocol: None,
            capabilities: BTreeMap::new(),
            current_state: ExpressionAdapterState::default(),
        }
    }

    pub fn capability_for(
        &self,
        action: &ExpressionTimelineAction,
    ) -> Option<&ExpressionActionCapability> {
        self.capabilities.get(&action.action).or_else(|| {
            self.capabilities
                .get(&format!("{}.{}", action.channel, action.action))
        })
    }

    pub fn supports(&self, action: &ExpressionTimelineAction) -> bool {
        self.capability_for(action)
            .map(|capability| capability.supported)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionActionCapability {
    #[serde(default = "default_true")]
    pub supported: bool,
    #[serde(default)]
    pub params: Vec<String>,
    #[serde(default)]
    pub safety_class: Option<ExpressionSafetyClass>,
    #[serde(default)]
    pub max_intensity: Option<f32>,
    #[serde(default)]
    pub max_duration_ms: Option<u32>,
    #[serde(default)]
    pub max_frequency_per_min: Option<u32>,
    #[serde(default)]
    pub interruptible: Option<bool>,
}

impl Default for ExpressionActionCapability {
    fn default() -> Self {
        Self {
            supported: true,
            params: Vec::new(),
            safety_class: None,
            max_intensity: None,
            max_duration_ms: None,
            max_frequency_per_min: None,
            interruptible: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExpressionAdapterState {
    #[serde(default)]
    pub busy: bool,
    #[serde(default)]
    pub thermal_limit: bool,
    #[serde(default)]
    pub battery_saver: bool,
    #[serde(default)]
    pub battery_level: Option<f32>,
    #[serde(default)]
    pub motion_adapter_registered: bool,
}

fn default_true() -> bool {
    true
}

fn default_dialogue_act() -> String {
    "respond".to_string()
}

fn default_content_tone() -> String {
    "neutral".to_string()
}
