use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::Duration as StdDuration;

use crate::config::{
    DialoguePolicyResourceSection, ExpressionPolicySection, ExpressionStyleSection,
    RegulationPolicySection, ServerConfig,
};
use crate::providers::adapter::TtsCapabilities;
use chrono::{DateTime, Duration, Utc};
use sea_orm::DatabaseConnection;

use super::abi::{ExternalExpressionPlugin, load_external_expression_plugins};
use super::affect::{AffectAssistantResponse, AffectObservation, AffectStore, AffectUserInput};
use super::appraisal::{
    AppraisalContext, AppraisalEnginePlugin, RuleBasedAppraisalEngine,
    TextClassifierAppraisalEngine,
};
use super::extension::{
    AffectReducerContext, AffectReducerPlugin, ExpressionExtensionRegistry,
    ExpressionPlannerPlugin, ExpressionPlanningContext, ExpressionSafetyPolicyPlugin,
    safety_modification, validate_expression_plan,
};
use super::regulation::regulate_expression;
use super::types::{
    AffectSnapshot, ExpressionCapabilitySnapshot, ExpressionInput, ExpressionIntent,
    ExpressionPlan, ExpressionPriority, ExpressionSafetyClass, ExpressionTimelineAction,
    ExpressiveStance, FallbackPolicy, ProsodyControls, RelationshipClearResult,
    RelationshipProfile, RelationshipPruneResult, SpeechExpression,
};

#[derive(Debug)]
pub struct ExpressionRuntime {
    policy: ExpressionPolicySection,
    regulation: RegulationPolicySection,
    affect: AffectStore,
    styles: BTreeMap<String, ExpressionStyleSection>,
    dialogue_policy: DialoguePolicyResourceSection,
    extensions: ExpressionExtensionRegistry,
    safety_state: ExpressionSafetyRuntimeState,
}

#[derive(Debug, Default)]
struct ExpressionSafetyRuntimeState {
    action_history: RwLock<BTreeMap<String, Vec<DateTime<Utc>>>>,
}

impl ExpressionRuntime {
    pub fn from_config(config: &ServerConfig) -> Self {
        let runtime = Self {
            policy: config.policies.expression.clone(),
            regulation: config.policies.regulation.clone(),
            affect: AffectStore::new(config.policies.affect.clone()),
            styles: config.profiles.expression_styles.clone(),
            dialogue_policy: config.resources.dialogue_policy.clone(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };
        if let Err(error) = runtime.register_appraisal_engine(Arc::new(
            TextClassifierAppraisalEngine::new(config.resources.appraisal_lexicons.clone()),
        )) {
            tracing::warn!(
                error = ?error,
                "failed to register built-in classifier appraisal engine"
            );
        }
        match load_external_expression_plugins(config) {
            Ok(plugins) => {
                for plugin in plugins {
                    if let Err(error) = runtime.register_external_plugin(plugin) {
                        tracing::warn!(
                            error = ?error,
                            "failed to register external expression plugin from manifest"
                        );
                    }
                }
            }
            Err(error) => tracing::warn!(
                error = ?error,
                "failed to load external expression plugin manifests"
            ),
        }
        runtime
    }

    pub async fn attach_db(&self, db: DatabaseConnection) -> anyhow::Result<()> {
        self.affect.attach_db(db).await
    }

    pub fn dialogue_policy(&self) -> &DialoguePolicyResourceSection {
        &self.dialogue_policy
    }

    pub async fn observe_user_input(&self, input: AffectUserInput<'_>) -> AffectObservation {
        let now = Utc::now();
        let ctx = AppraisalContext {
            channel_id: input.channel_id,
            user_id: input.user_id,
            user_text: input.user_text,
            now,
            policy: self.affect.policy(),
        };
        let rule_engine = RuleBasedAppraisalEngine;
        let mut appraisals = match rule_engine.appraise_user_input(&ctx).await {
            Ok(appraisals) => appraisals,
            Err(error) => {
                tracing::warn!(
                    error = ?error,
                    "built-in rule appraisal engine failed; continuing with plugin appraisals"
                );
                Vec::new()
            }
        };
        appraisals.extend(
            self.extensions
                .appraise_user_input(&ctx, self.external_plugin_timeout())
                .await,
        );

        let mut observation = self.affect.observe_user_appraisals(input, appraisals);
        self.extensions
            .apply_reducers(
                &AffectReducerContext {
                    channel_id: input.channel_id,
                    user_id: input.user_id,
                    route: None,
                },
                &mut observation.snapshot,
                self.external_plugin_timeout(),
            )
            .await;
        observation
    }

    pub async fn observe_assistant_response(
        &self,
        input: AffectAssistantResponse<'_>,
    ) -> AffectObservation {
        let mut observation = self.affect.observe_assistant_response(input);
        self.extensions
            .apply_reducers(
                &AffectReducerContext {
                    channel_id: input.channel_id,
                    user_id: input.user_id,
                    route: Some(input.route),
                },
                &mut observation.snapshot,
                self.external_plugin_timeout(),
            )
            .await;
        observation
    }

    pub async fn affect_snapshot(&self, channel_id: &str, user_id: &str) -> AffectSnapshot {
        self.affect_snapshot_with_route(channel_id, user_id, None)
            .await
    }

    pub fn relationship_profile(&self, user_id: &str) -> RelationshipProfile {
        self.affect.relationship_profile(user_id)
    }

    pub async fn clear_relationship_data(
        &self,
        user_id: &str,
    ) -> anyhow::Result<RelationshipClearResult> {
        self.affect.clear_relationship_data(user_id).await
    }

    pub fn prune_relationship_profiles(&self) -> RelationshipPruneResult {
        self.affect.prune_relationship_profiles()
    }

    pub fn register_affect_reducer(
        &self,
        plugin: Arc<dyn AffectReducerPlugin>,
    ) -> anyhow::Result<()> {
        self.extensions.register_reducer(plugin)
    }

    pub fn register_external_plugin(
        &self,
        plugin: Arc<dyn ExternalExpressionPlugin>,
    ) -> anyhow::Result<()> {
        self.extensions.register_external_plugin(plugin)
    }

    pub fn unregister_external_plugin(&self, name: &str) -> bool {
        self.extensions.unregister_external_plugin(name)
    }

    pub fn register_appraisal_engine(
        &self,
        plugin: Arc<dyn AppraisalEnginePlugin>,
    ) -> anyhow::Result<()> {
        self.extensions.register_appraisal_engine(plugin)
    }

    pub fn unregister_appraisal_engine(&self, name: &str) -> bool {
        self.extensions.unregister_appraisal_engine(name)
    }

    pub fn unregister_affect_reducer(&self, name: &str) -> bool {
        self.extensions.unregister_reducer(name)
    }

    pub fn register_expression_planner(
        &self,
        plugin: Arc<dyn ExpressionPlannerPlugin>,
    ) -> anyhow::Result<()> {
        self.extensions.register_planner(plugin)
    }

    pub fn unregister_expression_planner(&self, name: &str) -> bool {
        self.extensions.unregister_planner(name)
    }

    pub fn register_expression_safety_policy(
        &self,
        plugin: Arc<dyn ExpressionSafetyPolicyPlugin>,
    ) -> anyhow::Result<()> {
        self.extensions.register_safety_policy(plugin)
    }

    pub fn unregister_expression_safety_policy(&self, name: &str) -> bool {
        self.extensions.unregister_safety_policy(name)
    }

    pub fn extension_summary(&self) -> super::extension::ExpressionExtensionSummary {
        self.extensions.summary()
    }

    pub async fn plan(&self, input: ExpressionInput) -> ExpressionPlan {
        if !self.policy.enabled {
            return self.disabled_plan();
        }

        let affect = self
            .affect_snapshot_with_route(&input.channel_id, &input.user_id, input.route.as_deref())
            .await;
        let mut intent = self.plan_intent(&input, &affect);
        self.extensions
            .apply_intent_planners(&input, &affect, &mut intent, self.external_plugin_timeout())
            .await;
        let stance = regulate_expression(
            &affect,
            &intent.social_function,
            &intent.style,
            &self.regulation,
        );
        let mut plan = self.realize_intent(&input, &intent, &stance);

        let ctx = ExpressionPlanningContext {
            input: &input,
            affect: &affect,
            stance: &stance,
        };
        self.extensions
            .apply_planners(&ctx, &mut plan, self.external_plugin_timeout())
            .await;
        self.extensions
            .apply_safety_policies(&ctx, &mut plan, self.external_plugin_timeout())
            .await;
        apply_builtin_safety_policy(&self.policy, &self.safety_state, &input, &mut plan);
        if let Err(error) = validate_expression_plan(&plan) {
            tracing::warn!(
                error = ?error,
                plan_id = %plan.id,
                "expression plan validation failed after plugins; clamping unsafe values"
            );
            clamp_expression_plan(&mut plan);
        }
        plan
    }

    fn plan_intent(&self, input: &ExpressionInput, affect: &AffectSnapshot) -> ExpressionIntent {
        let mut intent = base_intent_from_dialogue_act(&input.dialogue_act);

        let dialogue_act = input.dialogue_act.trim().to_ascii_lowercase();
        match dialogue_act.as_str() {
            "apologize" | "apology" => {
                intent.intent = "respectful_apology".to_string();
                intent.social_function = "apologize".to_string();
                intent.style = "gentle".to_string();
                intent.intensity = intent.intensity.min(0.34);
            }
            "warn" | "warning" => {
                intent.intent = "clear_warning".to_string();
                intent.social_function = "warn".to_string();
                intent.style = "serious".to_string();
                intent.intensity = intent.intensity.max(0.42);
                intent.priority = ExpressionPriority::High;
                intent.safety_class = ExpressionSafetyClass::MediumRisk;
            }
            "explain" | "technical_explain" => {
                intent.intent = "calm_explanation".to_string();
                intent.social_function = "explain".to_string();
                intent.style = "calm".to_string();
                intent.intensity = intent.intensity.min(0.36);
            }
            "comfort" | "support" => {
                intent.intent = "gentle_comfort".to_string();
                intent.social_function = "comfort".to_string();
                intent.style = "gentle".to_string();
                intent.intensity = intent.intensity.min(0.34);
            }
            "celebrate" => {
                intent.intent = "warm_celebration".to_string();
                intent.social_function = "celebrate".to_string();
                intent.style = "cheerful".to_string();
                intent.intensity = intent.intensity.max(0.48);
            }
            "decline" | "refuse" => {
                intent.intent = "respectful_decline".to_string();
                intent.social_function = "decline".to_string();
                intent.style = "gentle".to_string();
                intent.intensity = intent.intensity.min(0.3);
            }
            "focus" | "tool_result" => {
                intent.intent = "focused_response".to_string();
                intent.social_function = "focus".to_string();
                intent.style = "calm".to_string();
                intent.intensity = intent.intensity.min(0.28);
            }
            _ => {}
        }

        let content_tone = input.content_tone.trim().to_ascii_lowercase();
        match content_tone.as_str() {
            "technical" | "precise" => {
                if !matches!(intent.social_function.as_str(), "warn" | "apologize") {
                    intent.social_function = "explain".to_string();
                    intent.style = "calm".to_string();
                    intent.intent = "calm_explanation".to_string();
                }
                intent.intensity = intent.intensity.min(0.36);
            }
            "serious" | "safety" => {
                intent.style = "serious".to_string();
                intent.intensity = intent.intensity.min(0.4);
            }
            "concise" | "minimal" => {
                intent.social_function = "focus".to_string();
                intent.style = "calm".to_string();
                intent.intent = "minimal_clear_response".to_string();
                intent.intensity = intent.intensity.min(0.24);
            }
            "playful" => {
                if input.user_state.frustration < 0.35 && !input.safety_context.active_task_serious
                {
                    intent.style = "cheerful".to_string();
                    intent.intensity = (intent.intensity + 0.06).min(0.55);
                }
            }
            _ => {}
        }

        if input.conversation_context.response_was_fallback {
            intent.intent = "respectful_recovery".to_string();
            intent.social_function = "apologize".to_string();
            intent.style = "gentle".to_string();
            intent.intensity = intent.intensity.min(0.3);
        }

        if input.conversation_context.used_tools || input.constraints.require_clarity {
            if !matches!(
                intent.social_function.as_str(),
                "warn" | "apologize" | "decline"
            ) {
                intent.intent = "focused_response".to_string();
                intent.social_function = "focus".to_string();
                intent.style = "calm".to_string();
                intent.intensity = intent.intensity.min(0.3);
            }
        }

        if !input.constraints.allow_playful && intent.social_function == "celebrate" {
            intent.intent = "warm_response".to_string();
            intent.social_function = "acknowledge".to_string();
            intent.style = "gentle".to_string();
            intent.intensity = intent.intensity.min(0.34);
        }

        if let Some(max_intensity) = input.constraints.max_intensity {
            intent.intensity = intent.intensity.min(max_intensity.clamp(0.0, 1.0));
        }

        if input.user_state.frustration >= 0.45 {
            intent.intent = "calm_supportive_response".to_string();
            intent.social_function = if intent.social_function == "apologize" {
                "apologize".to_string()
            } else {
                "focus".to_string()
            };
            intent.style = "calm".to_string();
            intent.intensity = intent.intensity.min(0.28);
        }

        if input.safety_context.active_task_serious {
            intent.style = if intent.social_function == "warn" {
                "serious".to_string()
            } else {
                "calm".to_string()
            };
            intent.intensity = intent.intensity.min(0.32);
        }

        if input.constraints.prefer_minimal
            || input.safety_context.minimal_expression
            || input.safety_context.night_mode
        {
            intent.intent = "minimal_clear_response".to_string();
            intent.social_function = "focus".to_string();
            intent.style = "calm".to_string();
            intent.intensity = intent.intensity.min(0.22);
        }

        if affect.resource.fatigue > 0.65 {
            intent.intensity = intent.intensity.min(0.26);
        }

        intent.intensity = intent.intensity.clamp(0.0, 1.0);
        intent
    }

    fn realize_intent(
        &self,
        input: &ExpressionInput,
        intent: &ExpressionIntent,
        stance: &ExpressiveStance,
    ) -> ExpressionPlan {
        let style_name = self.resolve_style_name(intent, stance);
        let style = self.styles.get(&style_name);
        let prosody = apply_stance_to_prosody(
            style
                .map(ExpressionStyleSection::prosody)
                .unwrap_or_else(ProsodyControls::default),
            &stance,
        );
        let voice_profile = style.and_then(|style| style.voice_profile.clone());

        let intensity = expression_intensity(intent.intensity, &stance, &self.policy);
        let speech = input.voice_output.then(|| {
            let speech = SpeechExpression {
                style: style_name.clone(),
                intensity,
                voice_profile,
                prosody,
            };
            input
                .tts_capabilities
                .as_ref()
                .map(|capabilities| adapt_speech_to_tts_capabilities(speech.clone(), capabilities))
                .unwrap_or(speech)
        });

        let device_context = input.effective_device_context();
        let timeline = if self.policy.auto_motion_enabled
            && input.hardware_control
            && device_context.supports_motion
            && input.safety_context.motion_allowed
            && !input.safety_context.minimal_expression
        {
            motion_timeline(
                &intent.social_function,
                intensity * stance.motion_scale,
                input.expression_capabilities.as_ref(),
                &self.policy,
            )
        } else {
            Vec::new()
        };

        ExpressionPlan {
            id: uuid::Uuid::new_v4(),
            intent: intent.intent.clone(),
            social_function: intent.social_function.clone(),
            style: style_name,
            intensity,
            stance: stance.clone(),
            priority: intent.priority,
            interruptible: intent.interruptible,
            safety_class: intent.safety_class,
            speech,
            timeline,
            safety_modifications: Vec::new(),
            fallback: FallbackPolicy::default(),
        }
    }

    pub async fn speech_for_text(&self, text: &str) -> SpeechExpression {
        self.speech_for_text_for_user(text, "", "", "respond", "neutral")
            .await
    }

    pub async fn speech_for_text_for_user(
        &self,
        text: &str,
        channel_id: &str,
        user_id: &str,
        dialogue_act: &str,
        content_tone: &str,
    ) -> SpeechExpression {
        self.plan(ExpressionInput {
            response_text: text.to_string(),
            route: None,
            channel_id: channel_id.to_string(),
            user_id: user_id.to_string(),
            dialogue_act: dialogue_act.to_string(),
            content_tone: content_tone.to_string(),
            user_state: Default::default(),
            device_context: Default::default(),
            safety_context: Default::default(),
            conversation_context: Default::default(),
            constraints: Default::default(),
            voice_output: true,
            hardware_control: false,
            tts_capabilities: None,
            expression_capabilities: None,
        })
        .await
        .speech
        .unwrap_or_else(|| SpeechExpression {
            style: self.policy.default_style.clone(),
            intensity: 0.3,
            voice_profile: self
                .styles
                .get(&self.policy.default_style)
                .and_then(|style| style.voice_profile.clone()),
            prosody: self
                .styles
                .get(&self.policy.default_style)
                .map(ExpressionStyleSection::prosody)
                .unwrap_or_default(),
        })
    }

    fn resolve_style_name(&self, intent: &ExpressionIntent, stance: &ExpressiveStance) -> String {
        let mut candidate = stance.style_hint.as_deref().unwrap_or(&intent.style);
        if !matches!(intent.social_function.as_str(), "warn" | "apologize") {
            if stance.expressed_valence > 0.22 && stance.warmth > 0.65 {
                candidate = "cheerful";
            } else if stance.expressed_arousal < 0.34 && stance.strategy != "careful_warmth" {
                candidate = "calm";
            }
        }

        if self.styles.contains_key(candidate) {
            candidate.to_string()
        } else if self.styles.contains_key(&self.policy.default_style) {
            self.policy.default_style.clone()
        } else {
            "neutral".to_string()
        }
    }

    fn disabled_plan(&self) -> ExpressionPlan {
        ExpressionPlan {
            id: uuid::Uuid::new_v4(),
            intent: "none".to_string(),
            social_function: "none".to_string(),
            style: "neutral".to_string(),
            intensity: 0.0,
            stance: ExpressiveStance {
                strategy: "disabled".to_string(),
                max_intensity: 0.0,
                motion_scale: 0.0,
                reasons: vec!["expression_disabled".to_string()],
                ..ExpressiveStance::default()
            },
            priority: ExpressionPriority::Low,
            interruptible: true,
            safety_class: ExpressionSafetyClass::LowRisk,
            speech: None,
            timeline: Vec::new(),
            safety_modifications: Vec::new(),
            fallback: FallbackPolicy::default(),
        }
    }

    fn external_plugin_timeout(&self) -> StdDuration {
        StdDuration::from_millis(self.policy.external_plugin_timeout_ms.max(1))
    }

    async fn affect_snapshot_with_route(
        &self,
        channel_id: &str,
        user_id: &str,
        route: Option<&str>,
    ) -> AffectSnapshot {
        let mut snapshot = self.affect.snapshot(channel_id, user_id);
        self.extensions
            .apply_reducers(
                &AffectReducerContext {
                    channel_id,
                    user_id,
                    route,
                },
                &mut snapshot,
                self.external_plugin_timeout(),
            )
            .await;
        snapshot
    }
}

fn base_intent_from_dialogue_act(dialogue_act: &str) -> ExpressionIntent {
    match dialogue_act.trim().to_ascii_lowercase().as_str() {
        "apologize" | "apology" => ExpressionIntent {
            intent: "respectful_apology".to_string(),
            social_function: "apologize".to_string(),
            style: "gentle".to_string(),
            intensity: 0.32,
            priority: ExpressionPriority::Normal,
            interruptible: true,
            safety_class: ExpressionSafetyClass::LowRisk,
        },
        "warn" | "warning" => ExpressionIntent {
            intent: "clear_warning".to_string(),
            social_function: "warn".to_string(),
            style: "serious".to_string(),
            intensity: 0.45,
            priority: ExpressionPriority::High,
            interruptible: true,
            safety_class: ExpressionSafetyClass::MediumRisk,
        },
        "celebrate" => ExpressionIntent {
            intent: "warm_celebration".to_string(),
            social_function: "celebrate".to_string(),
            style: "cheerful".to_string(),
            intensity: 0.52,
            priority: ExpressionPriority::Normal,
            interruptible: true,
            safety_class: ExpressionSafetyClass::LowRisk,
        },
        "invite" | "ask" => ExpressionIntent {
            intent: "attentive_invitation".to_string(),
            social_function: "invite".to_string(),
            style: "gentle".to_string(),
            intensity: 0.36,
            priority: ExpressionPriority::Normal,
            interruptible: true,
            safety_class: ExpressionSafetyClass::LowRisk,
        },
        "explain" | "technical_explain" => ExpressionIntent {
            intent: "calm_explanation".to_string(),
            social_function: "explain".to_string(),
            style: "calm".to_string(),
            intensity: 0.34,
            priority: ExpressionPriority::Normal,
            interruptible: true,
            safety_class: ExpressionSafetyClass::LowRisk,
        },
        "comfort" | "support" => ExpressionIntent {
            intent: "gentle_comfort".to_string(),
            social_function: "comfort".to_string(),
            style: "gentle".to_string(),
            intensity: 0.32,
            priority: ExpressionPriority::Normal,
            interruptible: true,
            safety_class: ExpressionSafetyClass::LowRisk,
        },
        "decline" | "refuse" => ExpressionIntent {
            intent: "respectful_decline".to_string(),
            social_function: "decline".to_string(),
            style: "gentle".to_string(),
            intensity: 0.3,
            priority: ExpressionPriority::Normal,
            interruptible: true,
            safety_class: ExpressionSafetyClass::LowRisk,
        },
        "focus" | "tool_result" => ExpressionIntent {
            intent: "focused_response".to_string(),
            social_function: "focus".to_string(),
            style: "calm".to_string(),
            intensity: 0.28,
            priority: ExpressionPriority::Normal,
            interruptible: true,
            safety_class: ExpressionSafetyClass::LowRisk,
        },
        _ => ExpressionIntent {
            intent: "warm_response".to_string(),
            social_function: "acknowledge".to_string(),
            style: "gentle".to_string(),
            intensity: 0.38,
            priority: ExpressionPriority::Normal,
            interruptible: true,
            safety_class: ExpressionSafetyClass::LowRisk,
        },
    }
}

fn expression_intensity(
    base: f32,
    stance: &ExpressiveStance,
    policy: &ExpressionPolicySection,
) -> f32 {
    let arousal_offset = stance.expressed_arousal - 0.35;
    let adjusted = base + stance.expressed_valence.max(0.0) * 0.08
        - (-stance.expressed_valence).max(0.0) * 0.04
        + arousal_offset * 0.08
        + (stance.warmth - 0.55) * 0.04;

    (adjusted * stance.intensity_scale)
        .min(policy.max_expression_intensity)
        .min(stance.max_intensity)
        .clamp(0.0, 1.0)
}

fn apply_stance_to_prosody(
    mut prosody: ProsodyControls,
    stance: &ExpressiveStance,
) -> ProsodyControls {
    let arousal_offset = stance.expressed_arousal - 0.35;
    prosody.rate = (prosody.rate + arousal_offset * 0.06 + stance.rate_offset).clamp(0.5, 2.0);
    prosody.pitch_shift = (prosody.pitch_shift + stance.expressed_valence * 0.025).clamp(-1.0, 1.0);
    prosody.energy = (prosody.energy
        + stance.expressed_valence * 0.04
        + arousal_offset * 0.08
        + (stance.warmth - 0.55) * 0.03
        + stance.energy_offset)
        .clamp(0.0, 1.0);
    prosody.pause_bias = (prosody.pause_bias
        + (-stance.expressed_valence).max(0.0) * 0.04
        + stance.pause_bias_offset)
        .clamp(-1.0, 1.0);
    prosody
}

fn adapt_speech_to_tts_capabilities(
    mut speech: SpeechExpression,
    capabilities: &TtsCapabilities,
) -> SpeechExpression {
    if !capabilities.supports_style {
        speech.style = "neutral".to_string();
    }
    if !capabilities.supports_intensity {
        speech.intensity = 0.0;
    }
    if !capabilities.supports_reference_voice {
        speech.voice_profile = None;
    }
    if !capabilities.supports_rate {
        speech.prosody.rate = 1.0;
    } else {
        speech.prosody.rate = speech.prosody.rate.clamp(
            capabilities.prosody_limits.min_rate,
            capabilities.prosody_limits.max_rate,
        );
    }
    if !capabilities.supports_pitch {
        speech.prosody.pitch_shift = 0.0;
    } else {
        speech.prosody.pitch_shift = speech.prosody.pitch_shift.clamp(
            capabilities.prosody_limits.min_pitch_shift,
            capabilities.prosody_limits.max_pitch_shift,
        );
    }
    if !capabilities.supports_energy {
        speech.prosody.energy = 0.5;
    } else {
        speech.prosody.energy = speech.prosody.energy.clamp(
            capabilities.prosody_limits.min_energy,
            capabilities.prosody_limits.max_energy,
        );
    }
    if !capabilities.supports_pause {
        speech.prosody.pause_bias = 0.0;
    } else {
        speech.prosody.pause_bias = speech.prosody.pause_bias.clamp(
            capabilities.prosody_limits.min_pause_bias,
            capabilities.prosody_limits.max_pause_bias,
        );
    }
    speech
}

fn motion_timeline(
    social_function: &str,
    intensity: f32,
    capabilities: Option<&ExpressionCapabilitySnapshot>,
    policy: &ExpressionPolicySection,
) -> Vec<ExpressionTimelineAction> {
    let intensity = intensity
        .min(policy.max_motion_intensity)
        .clamp(0.0, policy.max_motion_intensity.max(0.0));
    if intensity <= 0.05 || social_function == "warn" {
        return Vec::new();
    }

    let action = match social_function {
        "apologize" => "small_bow",
        "celebrate" => "small_nod",
        "invite" => "look_at_user",
        _ => "small_nod",
    };

    let timeline_action = ExpressionTimelineAction {
        at: "speech_start".to_string(),
        channel: "motion".to_string(),
        action: action.to_string(),
        intensity,
        duration_ms: 800,
        params: serde_json::json!({}),
    };

    match capabilities {
        Some(capabilities) if capabilities.supports(&timeline_action) => vec![timeline_action],
        _ => Vec::new(),
    }
}

fn apply_builtin_safety_policy(
    policy: &ExpressionPolicySection,
    safety_state: &ExpressionSafetyRuntimeState,
    input: &ExpressionInput,
    plan: &mut ExpressionPlan,
) {
    let mut modifications = Vec::new();
    let safety = &input.safety_context;
    let device = input.effective_device_context();
    let expression_intensity_limit = policy
        .max_expression_intensity
        .min(
            safety
                .max_expression_intensity
                .unwrap_or(1.0)
                .clamp(0.0, 1.0),
        )
        .min(
            input
                .constraints
                .max_intensity
                .unwrap_or(1.0)
                .clamp(0.0, 1.0),
        );
    let motion_intensity_limit = policy
        .max_motion_intensity
        .min(safety.max_motion_intensity.unwrap_or(1.0).clamp(0.0, 1.0))
        .min(
            input
                .constraints
                .max_motion_intensity
                .unwrap_or(1.0)
                .clamp(0.0, 1.0),
        );
    let night_or_minimal =
        policy.night_mode_minimal_expression || safety.night_mode || safety.minimal_expression;

    let before_intensity = plan.intensity;
    plan.intensity = plan.intensity.clamp(0.0, expression_intensity_limit);
    if (plan.intensity - before_intensity).abs() > f32::EPSILON {
        modifications.push(safety_modification(
            "builtin_expression_safety",
            "intensity",
            "expression_intensity_cap",
            Some(serde_json::json!(before_intensity)),
            Some(serde_json::json!(plan.intensity)),
        ));
    }

    if let Some(speech) = &mut plan.speech {
        let before = speech.intensity;
        speech.intensity = speech.intensity.clamp(0.0, expression_intensity_limit);
        if night_or_minimal {
            speech.intensity = speech.intensity.min(0.25);
        }
        if (speech.intensity - before).abs() > f32::EPSILON {
            modifications.push(safety_modification(
                "builtin_expression_safety",
                "speech.intensity",
                if night_or_minimal {
                    "minimal_expression_speech_cap"
                } else {
                    "expression_intensity_cap"
                },
                Some(serde_json::json!(before)),
                Some(serde_json::json!(speech.intensity)),
            ));
        }

        if let Some(volume_limit) = safety.volume_limit {
            let before = speech.prosody.energy;
            speech.prosody.energy = speech.prosody.energy.min(volume_limit.clamp(0.0, 1.0));
            if (speech.prosody.energy - before).abs() > f32::EPSILON {
                modifications.push(safety_modification(
                    "builtin_expression_safety",
                    "speech.prosody.energy",
                    "volume_limit_energy_cap",
                    Some(serde_json::json!(before)),
                    Some(serde_json::json!(speech.prosody.energy)),
                ));
            }
        }
    }

    if night_or_minimal && !plan.timeline.is_empty() {
        let before = serde_json::json!(plan.timeline.len());
        plan.timeline.clear();
        modifications.push(safety_modification(
            "builtin_expression_safety",
            "timeline",
            if safety.night_mode {
                "night_mode_minimal_expression"
            } else {
                "minimal_expression"
            },
            Some(before),
            Some(serde_json::json!(0)),
        ));
        plan.safety_modifications.extend(modifications);
        return;
    }

    if !safety.motion_allowed
        && plan
            .timeline
            .iter()
            .any(|action| action.channel == "motion")
    {
        let before = serde_json::json!(plan.timeline.len());
        plan.timeline.retain(|action| action.channel != "motion");
        modifications.push(safety_modification(
            "builtin_expression_safety",
            "timeline",
            "motion_not_allowed",
            Some(before),
            Some(serde_json::json!(plan.timeline.len())),
        ));
    }

    let adapter_state = input
        .expression_capabilities
        .as_ref()
        .map(|capabilities| &capabilities.current_state);
    if device.thermal_limit
        || adapter_state
            .map(|state| state.busy || state.thermal_limit)
            .unwrap_or(false)
    {
        let before = serde_json::json!(plan.timeline.len());
        plan.timeline.clear();
        modifications.push(safety_modification(
            "builtin_expression_safety",
            "timeline",
            "adapter_unavailable",
            Some(before),
            Some(serde_json::json!(0)),
        ));
        plan.safety_modifications.extend(modifications);
        return;
    }

    let mut kept = Vec::with_capacity(plan.timeline.len());
    for (index, mut action) in plan.timeline.drain(..).enumerate() {
        let path = format!("timeline[{index}]");

        if action.channel == "motion" && !policy.allowed_motion_actions.contains(&action.action) {
            modifications.push(safety_modification(
                "builtin_expression_safety",
                path,
                "motion_action_not_allowlisted",
                Some(serde_json::json!(action.action)),
                None,
            ));
            continue;
        }

        if action.channel == "motion" && !device.supports_motion {
            modifications.push(safety_modification(
                "builtin_expression_safety",
                path,
                "device_motion_unavailable",
                Some(serde_json::json!(action.action)),
                None,
            ));
            continue;
        }

        if safety.active_task_serious
            && action.channel == "motion"
            && !matches!(plan.social_function.as_str(), "warn" | "apologize")
        {
            modifications.push(safety_modification(
                "builtin_expression_safety",
                path,
                "serious_task_motion_suppressed",
                Some(serde_json::json!(action.action)),
                None,
            ));
            continue;
        }

        let capability = input
            .expression_capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.capability_for(&action));
        if input.hardware_control
            && capability
                .map(|capability| !capability.supported)
                .unwrap_or(true)
        {
            modifications.push(safety_modification(
                "builtin_expression_safety",
                path,
                "action_not_supported_by_adapter",
                Some(serde_json::json!(action.action)),
                None,
            ));
            continue;
        }

        if let Some(capability) = capability {
            if capability
                .safety_class
                .map(|safety_class| risk_rank(safety_class) > risk_rank(plan.safety_class))
                .unwrap_or(false)
            {
                modifications.push(safety_modification(
                    "builtin_expression_safety",
                    path,
                    "action_safety_class_exceeds_plan",
                    Some(serde_json::json!(action.action)),
                    None,
                ));
                continue;
            }

            if plan.interruptible && capability.interruptible == Some(false) {
                modifications.push(safety_modification(
                    "builtin_expression_safety",
                    path,
                    "non_interruptible_action_removed",
                    Some(serde_json::json!(action.action)),
                    None,
                ));
                continue;
            }

            if let Some(max_frequency_per_min) = capability.max_frequency_per_min {
                if !safety_state.reserve_action_slot(input, &action, max_frequency_per_min) {
                    modifications.push(safety_modification(
                        "builtin_expression_safety",
                        path,
                        "max_frequency_per_min",
                        Some(serde_json::json!({
                            "action": action.action,
                            "limit": max_frequency_per_min,
                        })),
                        None,
                    ));
                    continue;
                }
            }
        }

        let before_intensity = action.intensity;
        action.intensity = action.intensity.min(motion_intensity_limit).clamp(0.0, 1.0);
        if let Some(max_intensity) = capability.and_then(|capability| capability.max_intensity) {
            action.intensity = action.intensity.min(max_intensity.clamp(0.0, 1.0));
        }
        let battery_saver = device.battery_saver
            || device
                .battery_level
                .map(|battery| battery <= 0.15)
                .unwrap_or(false)
            || adapter_state
                .map(|state| state.battery_saver)
                .unwrap_or(false);
        if battery_saver {
            action.intensity = action.intensity.min(0.25);
        }
        if (action.intensity - before_intensity).abs() > f32::EPSILON {
            modifications.push(safety_modification(
                "builtin_expression_safety",
                format!("{path}.intensity"),
                if battery_saver {
                    "battery_saver_intensity_cap"
                } else {
                    "motion_intensity_cap"
                },
                Some(serde_json::json!(before_intensity)),
                Some(serde_json::json!(action.intensity)),
            ));
        }

        let duration_limit = capability
            .and_then(|capability| capability.max_duration_ms)
            .unwrap_or(policy.max_action_duration_ms)
            .min(policy.max_action_duration_ms);
        if action.duration_ms > duration_limit {
            let before = action.duration_ms;
            action.duration_ms = duration_limit;
            modifications.push(safety_modification(
                "builtin_expression_safety",
                format!("{path}.duration_ms"),
                "action_duration_cap",
                Some(serde_json::json!(before)),
                Some(serde_json::json!(action.duration_ms)),
            ));
        }

        kept.push(action);
    }

    if kept.len() > policy.max_timeline_actions {
        let before = kept.len();
        kept.truncate(policy.max_timeline_actions);
        modifications.push(safety_modification(
            "builtin_expression_safety",
            "timeline",
            "max_timeline_actions",
            Some(serde_json::json!(before)),
            Some(serde_json::json!(kept.len())),
        ));
    }
    plan.timeline = kept;
    plan.safety_modifications.extend(modifications);
}

fn clamp_expression_plan(plan: &mut ExpressionPlan) {
    plan.intensity = plan.intensity.clamp(0.0, 1.0);
    if let Some(speech) = &mut plan.speech {
        speech.intensity = speech.intensity.clamp(0.0, 1.0);
    }
    for action in &mut plan.timeline {
        action.intensity = action.intensity.clamp(0.0, 1.0);
    }
}

impl ExpressionSafetyRuntimeState {
    fn reserve_action_slot(
        &self,
        input: &ExpressionInput,
        action: &ExpressionTimelineAction,
        max_frequency_per_min: u32,
    ) -> bool {
        if max_frequency_per_min == 0 {
            return false;
        }

        let now = Utc::now();
        let cutoff = now - Duration::seconds(60);
        let key = format!(
            "{}:{}:{}:{}",
            input.channel_id, input.user_id, action.channel, action.action
        );
        let mut history = self
            .action_history
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entries = history.entry(key).or_default();
        entries.retain(|timestamp| *timestamp >= cutoff);
        if entries.len() >= max_frequency_per_min as usize {
            return false;
        }
        entries.push(now);
        true
    }
}

fn risk_rank(safety_class: ExpressionSafetyClass) -> u8 {
    match safety_class {
        ExpressionSafetyClass::LowRisk => 0,
        ExpressionSafetyClass::MediumRisk => 1,
        ExpressionSafetyClass::HighRisk => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AffectPolicySection, ExpressionStyleSection};
    use crate::expression::{
        AffectDeltas, AppraisalDecay, AppraisalDecayKind, AppraisalEvent, AppraisalScope,
        AppraisalSource, AppraisalTarget, ExpressionActionCapability, ExpressionAdapterState,
        ExpressionPluginCapabilities, ExpressionPluginManifest, ExpressionPluginRequest,
        ExpressionPluginResponse, ExternalAppraisalEventDraft, ExternalExpressionPlugin,
    };
    use async_trait::async_trait;

    #[test]
    fn dialogue_act_maps_apology_to_gentle_intent() {
        let classification = base_intent_from_dialogue_act("apologize");
        assert_eq!(classification.social_function, "apologize");
        assert_eq!(classification.style, "gentle");
    }

    #[tokio::test]
    async fn dialogue_act_and_content_tone_steer_intent_before_realization() {
        let mut styles = BTreeMap::new();
        styles.insert("cheerful".to_string(), ExpressionStyleSection::default());
        styles.insert("calm".to_string(), ExpressionStyleSection::default());
        styles.insert("neutral".to_string(), ExpressionStyleSection::default());

        let runtime = ExpressionRuntime {
            policy: ExpressionPolicySection::default(),
            regulation: RegulationPolicySection::default(),
            affect: AffectStore::new(AffectPolicySection::default()),
            styles,
            dialogue_policy: DialoguePolicyResourceSection::default(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };

        let mut input =
            test_expression_input("Great, this API behavior can be explained this way.");
        input.dialogue_act = "explain".to_string();
        input.content_tone = "technical".to_string();

        let plan = runtime.plan(input).await;

        assert_eq!(plan.intent, "calm_explanation");
        assert_eq!(plan.social_function, "explain");
        assert_eq!(plan.style, "calm");
        assert!(plan.intensity < 0.42);
    }

    #[tokio::test]
    async fn affect_state_can_shift_expression_towards_cheerful() {
        let mut styles = BTreeMap::new();
        styles.insert("gentle".to_string(), ExpressionStyleSection::default());
        styles.insert("cheerful".to_string(), ExpressionStyleSection::default());
        styles.insert("neutral".to_string(), ExpressionStyleSection::default());

        let runtime = ExpressionRuntime {
            policy: ExpressionPolicySection::default(),
            regulation: RegulationPolicySection::default(),
            affect: AffectStore::new(AffectPolicySection::default()),
            styles,
            dialogue_policy: DialoguePolicyResourceSection::default(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };

        for _ in 0..3 {
            runtime
                .observe_user_input(AffectUserInput {
                    channel_id: "cli",
                    user_id: "user",
                    user_text: "thank you, awesome",
                })
                .await;
            runtime
                .observe_assistant_response(AffectAssistantResponse {
                    channel_id: "cli",
                    user_id: "user",
                    response_text: "Great, I am happy I could help.",
                    route: "minor_llm",
                    used_tools: false,
                    response_was_fallback: false,
                })
                .await;
        }

        let plan = runtime
            .plan(ExpressionInput {
                response_text: "Okay.".to_string(),
                route: Some("minor_llm".to_string()),
                channel_id: "cli".to_string(),
                user_id: "user".to_string(),
                dialogue_act: "respond".to_string(),
                content_tone: "neutral".to_string(),
                user_state: Default::default(),
                device_context: Default::default(),
                safety_context: Default::default(),
                conversation_context: Default::default(),
                constraints: Default::default(),
                voice_output: true,
                hardware_control: false,
                tts_capabilities: None,
                expression_capabilities: None,
            })
            .await;

        assert_eq!(plan.style, "cheerful");
        assert!(plan.intensity > 0.38);
    }

    #[tokio::test]
    async fn regulation_prevents_negative_affect_from_overexpressing() {
        let mut styles = BTreeMap::new();
        styles.insert("gentle".to_string(), ExpressionStyleSection::default());
        styles.insert("calm".to_string(), ExpressionStyleSection::default());
        styles.insert("neutral".to_string(), ExpressionStyleSection::default());

        let runtime = ExpressionRuntime {
            policy: ExpressionPolicySection::default(),
            regulation: RegulationPolicySection::default(),
            affect: AffectStore::new(AffectPolicySection::default()),
            styles,
            dialogue_policy: DialoguePolicyResourceSection::default(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };

        for _ in 0..4 {
            runtime
                .observe_user_input(AffectUserInput {
                    channel_id: "cli",
                    user_id: "user",
                    user_text: "incorrect, you got it wrong",
                })
                .await;
        }

        let plan = runtime
            .plan(ExpressionInput {
                response_text: "I understand, let us look at it again.".to_string(),
                route: Some("minor_llm".to_string()),
                channel_id: "cli".to_string(),
                user_id: "user".to_string(),
                dialogue_act: "respond".to_string(),
                content_tone: "neutral".to_string(),
                user_state: Default::default(),
                device_context: Default::default(),
                safety_context: Default::default(),
                conversation_context: Default::default(),
                constraints: Default::default(),
                voice_output: true,
                hardware_control: true,
                tts_capabilities: None,
                expression_capabilities: None,
            })
            .await;

        assert!(
            plan.stance.expressed_valence
                >= -RegulationPolicySection::default().max_negative_expression
        );
        assert!(plan.intensity <= RegulationPolicySection::default().deescalated_max_intensity);
        assert!(
            plan.stance
                .reasons
                .iter()
                .any(|reason| reason == "negative_affect_regulation")
        );
    }

    #[tokio::test]
    async fn registered_reducer_can_contribute_to_expression_state() {
        struct WarmReducer;

        impl AffectReducerPlugin for WarmReducer {
            fn name(&self) -> &str {
                "test_warm_reducer"
            }

            fn reduce(
                &self,
                _ctx: &AffectReducerContext<'_>,
                snapshot: &mut AffectSnapshot,
            ) -> anyhow::Result<()> {
                snapshot.affect.valence = 0.48;
                snapshot.social.trust = 0.82;
                snapshot.social.familiarity = 0.4;
                Ok(())
            }
        }

        let mut styles = BTreeMap::new();
        styles.insert("gentle".to_string(), ExpressionStyleSection::default());
        styles.insert("cheerful".to_string(), ExpressionStyleSection::default());
        styles.insert("neutral".to_string(), ExpressionStyleSection::default());

        let runtime = ExpressionRuntime {
            policy: ExpressionPolicySection::default(),
            regulation: RegulationPolicySection::default(),
            affect: AffectStore::new(AffectPolicySection::default()),
            styles,
            dialogue_policy: DialoguePolicyResourceSection::default(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };
        runtime
            .register_affect_reducer(Arc::new(WarmReducer))
            .unwrap();

        let plan = runtime
            .plan(ExpressionInput {
                response_text: "Okay.".to_string(),
                route: Some("minor_llm".to_string()),
                channel_id: "cli".to_string(),
                user_id: "user".to_string(),
                dialogue_act: "respond".to_string(),
                content_tone: "neutral".to_string(),
                user_state: Default::default(),
                device_context: Default::default(),
                safety_context: Default::default(),
                conversation_context: Default::default(),
                constraints: Default::default(),
                voice_output: true,
                hardware_control: false,
                tts_capabilities: None,
                expression_capabilities: None,
            })
            .await;

        assert_eq!(plan.style, "cheerful");
        assert_eq!(
            runtime.extension_summary().reducers,
            vec!["test_warm_reducer".to_string()]
        );
    }

    #[tokio::test]
    async fn registered_appraisal_engine_can_contribute_events() {
        struct ClassifierEngine;

        #[async_trait]
        impl AppraisalEnginePlugin for ClassifierEngine {
            fn name(&self) -> &str {
                "test_classifier_appraisal"
            }

            async fn appraise_user_input(
                &self,
                ctx: &AppraisalContext<'_>,
            ) -> anyhow::Result<Vec<AppraisalEvent>> {
                Ok(vec![AppraisalEvent::new(
                    "classifier_warm_signal",
                    AppraisalSource::UserMessage,
                    AppraisalTarget::Dialogue,
                    0.91,
                    AppraisalScope::Session,
                    ctx.now,
                    ctx.policy,
                    AffectDeltas {
                        valence: 0.2,
                        engagement: 0.05,
                        ..AffectDeltas::default()
                    },
                    "Classifier detected warm user tone",
                )])
            }
        }

        let runtime = ExpressionRuntime {
            policy: ExpressionPolicySection::default(),
            regulation: RegulationPolicySection::default(),
            affect: AffectStore::new(AffectPolicySection::default()),
            styles: BTreeMap::new(),
            dialogue_policy: DialoguePolicyResourceSection::default(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };
        runtime
            .register_appraisal_engine(Arc::new(ClassifierEngine))
            .unwrap();

        let observation = runtime
            .observe_user_input(AffectUserInput {
                channel_id: "cli",
                user_id: "user",
                user_text: "just a normal sentence",
            })
            .await;

        assert!(
            observation
                .appraisals
                .iter()
                .any(|event| event.kind == "classifier_warm_signal")
        );
        assert!(
            observation.snapshot.affect.valence > AffectPolicySection::default().baseline_valence
        );
        assert_eq!(
            runtime.extension_summary().appraisal_engines,
            vec!["test_classifier_appraisal".to_string()]
        );
    }

    #[tokio::test]
    async fn external_abi_plugin_can_contribute_appraisal_and_intent() {
        struct AbiPlugin {
            manifest: ExpressionPluginManifest,
        }

        #[async_trait]
        impl ExternalExpressionPlugin for AbiPlugin {
            fn manifest(&self) -> &ExpressionPluginManifest {
                &self.manifest
            }

            async fn call(
                &self,
                request: ExpressionPluginRequest,
            ) -> anyhow::Result<ExpressionPluginResponse> {
                match request {
                    ExpressionPluginRequest::AppraiseUserInput { .. } => {
                        Ok(ExpressionPluginResponse::AppraisalEvents {
                            events: vec![ExternalAppraisalEventDraft {
                                kind: "external_warm_signal".to_string(),
                                source: AppraisalSource::UserMessage,
                                target: AppraisalTarget::Dialogue,
                                confidence: 0.9,
                                scope: AppraisalScope::Session,
                                deltas: AffectDeltas {
                                    valence: 0.2,
                                    engagement: 0.08,
                                    ..AffectDeltas::default()
                                },
                                evidence_summary: "External classifier detected warmth".to_string(),
                                ttl_sec: Some(u64::MAX),
                                decay: Some(AppraisalDecay {
                                    kind: AppraisalDecayKind::Exponential,
                                    half_life_sec: u64::MAX,
                                }),
                            }],
                        })
                    }
                    ExpressionPluginRequest::PlanIntent { mut intent, .. } => {
                        intent.intent = "external_intent".to_string();
                        intent.social_function = "comfort".to_string();
                        intent.style = "gentle".to_string();
                        intent.intensity = 0.26;
                        Ok(ExpressionPluginResponse::Intent { intent })
                    }
                    _ => Ok(ExpressionPluginResponse::Noop),
                }
            }
        }

        let mut styles = BTreeMap::new();
        styles.insert("gentle".to_string(), ExpressionStyleSection::default());
        styles.insert("neutral".to_string(), ExpressionStyleSection::default());

        let runtime = ExpressionRuntime {
            policy: ExpressionPolicySection::default(),
            regulation: RegulationPolicySection::default(),
            affect: AffectStore::new(AffectPolicySection::default()),
            styles,
            dialogue_policy: DialoguePolicyResourceSection::default(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };
        runtime
            .register_external_plugin(Arc::new(AbiPlugin {
                manifest: ExpressionPluginManifest {
                    name: "test_external_expression".to_string(),
                    enabled: true,
                    abi_version: 1,
                    request_timeout_ms: None,
                    trace_stderr: false,
                    max_stderr_bytes: 4096,
                    cancel_on_timeout: true,
                    sandbox: Default::default(),
                    command: Vec::new(),
                    working_dir: None,
                    capabilities: ExpressionPluginCapabilities {
                        appraisal_engine: true,
                        intent_planner: true,
                        ..ExpressionPluginCapabilities::default()
                    },
                },
            }))
            .unwrap();

        let observation = runtime
            .observe_user_input(AffectUserInput {
                channel_id: "cli",
                user_id: "user",
                user_text: "just a normal sentence",
            })
            .await;
        let plan = runtime
            .plan(test_expression_input("I will handle this gently."))
            .await;

        assert!(
            observation
                .appraisals
                .iter()
                .any(|event| event.kind == "external_warm_signal")
        );
        let external_event = observation
            .appraisals
            .iter()
            .find(|event| event.kind == "external_warm_signal")
            .expect("external plugin appraisal should be present");
        assert_eq!(
            external_event.ttl_sec,
            AffectPolicySection::default().session_appraisal_ttl_sec
        );
        assert_eq!(
            external_event.decay.half_life_sec,
            AffectPolicySection::default().appraisal_half_life_sec
        );
        assert_eq!(plan.intent, "external_intent");
        assert_eq!(
            runtime.extension_summary().external_plugins,
            vec!["test_external_expression".to_string()]
        );
    }

    #[tokio::test]
    async fn external_abi_plugin_timeout_is_recoverable() {
        struct SlowPlugin {
            manifest: ExpressionPluginManifest,
        }

        #[async_trait]
        impl ExternalExpressionPlugin for SlowPlugin {
            fn manifest(&self) -> &ExpressionPluginManifest {
                &self.manifest
            }

            async fn call(
                &self,
                _request: ExpressionPluginRequest,
            ) -> anyhow::Result<ExpressionPluginResponse> {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                Ok(ExpressionPluginResponse::AppraisalEvents {
                    events: vec![ExternalAppraisalEventDraft {
                        kind: "too_late".to_string(),
                        source: AppraisalSource::UserMessage,
                        target: AppraisalTarget::Dialogue,
                        confidence: 1.0,
                        scope: AppraisalScope::Session,
                        deltas: AffectDeltas {
                            valence: 0.5,
                            ..AffectDeltas::default()
                        },
                        evidence_summary: "This plugin should time out".to_string(),
                        ttl_sec: None,
                        decay: None,
                    }],
                })
            }
        }

        let runtime = ExpressionRuntime {
            policy: ExpressionPolicySection {
                external_plugin_timeout_ms: 1,
                ..ExpressionPolicySection::default()
            },
            regulation: RegulationPolicySection::default(),
            affect: AffectStore::new(AffectPolicySection::default()),
            styles: BTreeMap::new(),
            dialogue_policy: DialoguePolicyResourceSection::default(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };
        runtime
            .register_external_plugin(Arc::new(SlowPlugin {
                manifest: ExpressionPluginManifest {
                    name: "slow_external_expression".to_string(),
                    enabled: true,
                    abi_version: 1,
                    request_timeout_ms: None,
                    trace_stderr: false,
                    max_stderr_bytes: 4096,
                    cancel_on_timeout: true,
                    sandbox: Default::default(),
                    command: Vec::new(),
                    working_dir: None,
                    capabilities: ExpressionPluginCapabilities {
                        appraisal_engine: true,
                        ..ExpressionPluginCapabilities::default()
                    },
                },
            }))
            .unwrap();

        let observation = runtime
            .observe_user_input(AffectUserInput {
                channel_id: "cli",
                user_id: "user",
                user_text: "just a normal sentence",
            })
            .await;

        assert!(
            !observation
                .appraisals
                .iter()
                .any(|event| event.kind == "too_late")
        );
    }

    #[tokio::test]
    async fn motion_requires_negotiated_capability() {
        let mut styles = BTreeMap::new();
        styles.insert("cheerful".to_string(), ExpressionStyleSection::default());
        styles.insert("neutral".to_string(), ExpressionStyleSection::default());

        let runtime = ExpressionRuntime {
            policy: ExpressionPolicySection::default(),
            regulation: RegulationPolicySection::default(),
            affect: AffectStore::new(AffectPolicySection::default()),
            styles,
            dialogue_policy: DialoguePolicyResourceSection::default(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };

        let without_capability = runtime
            .plan(ExpressionInput {
                response_text: "Great, awesome!".to_string(),
                route: Some("minor_llm".to_string()),
                channel_id: "embedded".to_string(),
                user_id: "user".to_string(),
                dialogue_act: "respond".to_string(),
                content_tone: "neutral".to_string(),
                user_state: Default::default(),
                device_context: Default::default(),
                safety_context: Default::default(),
                conversation_context: Default::default(),
                constraints: Default::default(),
                voice_output: true,
                hardware_control: true,
                tts_capabilities: None,
                expression_capabilities: None,
            })
            .await;
        assert!(without_capability.timeline.is_empty());

        let with_capability = runtime
            .plan(ExpressionInput {
                response_text: "Great, awesome!".to_string(),
                route: Some("minor_llm".to_string()),
                channel_id: "embedded".to_string(),
                user_id: "user".to_string(),
                dialogue_act: "respond".to_string(),
                content_tone: "neutral".to_string(),
                user_state: Default::default(),
                device_context: Default::default(),
                safety_context: Default::default(),
                conversation_context: Default::default(),
                constraints: Default::default(),
                voice_output: true,
                hardware_control: true,
                tts_capabilities: None,
                expression_capabilities: Some(capabilities_with_action("small_nod", 0.3)),
            })
            .await;

        assert_eq!(with_capability.timeline.len(), 1);
        assert_eq!(with_capability.timeline[0].action, "small_nod");
        assert!(with_capability.timeline[0].intensity <= 0.3);
        assert!(
            with_capability
                .safety_modifications
                .iter()
                .any(|modification| modification.reason == "motion_intensity_cap")
        );
    }

    #[tokio::test]
    async fn builtin_safety_enforces_motion_frequency_limit() {
        let mut styles = BTreeMap::new();
        styles.insert("cheerful".to_string(), ExpressionStyleSection::default());
        styles.insert("neutral".to_string(), ExpressionStyleSection::default());

        let runtime = ExpressionRuntime {
            policy: ExpressionPolicySection::default(),
            regulation: RegulationPolicySection::default(),
            affect: AffectStore::new(AffectPolicySection::default()),
            styles,
            dialogue_policy: DialoguePolicyResourceSection::default(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };

        let mut capabilities = capabilities_with_action("small_nod", 0.8);
        capabilities
            .capabilities
            .get_mut("small_nod")
            .unwrap()
            .max_frequency_per_min = Some(1);

        let mut input = test_expression_input("Great, awesome!");
        input.channel_id = "embedded".to_string();
        input.hardware_control = true;
        input.expression_capabilities = Some(capabilities.clone());

        let first = runtime.plan(input.clone()).await;
        assert_eq!(first.timeline.len(), 1);

        let second = runtime.plan(input).await;
        assert!(second.timeline.is_empty());
        assert!(
            second
                .safety_modifications
                .iter()
                .any(|modification| modification.reason == "max_frequency_per_min")
        );
    }

    #[tokio::test]
    async fn builtin_safety_removes_actions_that_exceed_plan_risk_or_interruptibility() {
        let mut styles = BTreeMap::new();
        styles.insert("cheerful".to_string(), ExpressionStyleSection::default());
        styles.insert("neutral".to_string(), ExpressionStyleSection::default());

        let runtime = ExpressionRuntime {
            policy: ExpressionPolicySection::default(),
            regulation: RegulationPolicySection::default(),
            affect: AffectStore::new(AffectPolicySection::default()),
            styles,
            dialogue_policy: DialoguePolicyResourceSection::default(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };

        let mut risky_capabilities = capabilities_with_action("small_nod", 0.8);
        risky_capabilities
            .capabilities
            .get_mut("small_nod")
            .unwrap()
            .safety_class = Some(ExpressionSafetyClass::HighRisk);
        let mut risky_input = test_expression_input("Great, awesome!");
        risky_input.channel_id = "embedded".to_string();
        risky_input.hardware_control = true;
        risky_input.expression_capabilities = Some(risky_capabilities);

        let risky_plan = runtime.plan(risky_input).await;
        assert!(risky_plan.timeline.is_empty());
        assert!(
            risky_plan
                .safety_modifications
                .iter()
                .any(|modification| modification.reason == "action_safety_class_exceeds_plan")
        );

        let mut non_interruptible_capabilities = capabilities_with_action("small_nod", 0.8);
        non_interruptible_capabilities
            .capabilities
            .get_mut("small_nod")
            .unwrap()
            .interruptible = Some(false);
        let mut non_interruptible_input = test_expression_input("Great, awesome!");
        non_interruptible_input.channel_id = "embedded_2".to_string();
        non_interruptible_input.hardware_control = true;
        non_interruptible_input.expression_capabilities = Some(non_interruptible_capabilities);

        let non_interruptible_plan = runtime.plan(non_interruptible_input).await;
        assert!(non_interruptible_plan.timeline.is_empty());
        assert!(
            non_interruptible_plan
                .safety_modifications
                .iter()
                .any(|modification| modification.reason == "non_interruptible_action_removed")
        );
    }

    #[tokio::test]
    async fn safety_context_can_force_minimal_expression() {
        let mut styles = BTreeMap::new();
        styles.insert("cheerful".to_string(), ExpressionStyleSection::default());
        styles.insert("calm".to_string(), ExpressionStyleSection::default());
        styles.insert("neutral".to_string(), ExpressionStyleSection::default());

        let runtime = ExpressionRuntime {
            policy: ExpressionPolicySection::default(),
            regulation: RegulationPolicySection::default(),
            affect: AffectStore::new(AffectPolicySection::default()),
            styles,
            dialogue_policy: DialoguePolicyResourceSection::default(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };

        let mut input = test_expression_input("Great, awesome!");
        input.hardware_control = true;
        input.expression_capabilities = Some(capabilities_with_action("small_nod", 0.8));
        input.safety_context.minimal_expression = true;

        let plan = runtime.plan(input).await;

        assert_eq!(plan.intent, "minimal_clear_response");
        assert_eq!(plan.social_function, "focus");
        assert!(plan.timeline.is_empty());
        assert!(plan.speech.as_ref().unwrap().intensity <= 0.25);
    }

    #[tokio::test]
    async fn planner_and_safety_plugins_can_adjust_expression_plan() {
        struct MinimalPlanner;
        struct CapIntensityPolicy;

        impl ExpressionPlannerPlugin for MinimalPlanner {
            fn name(&self) -> &str {
                "test_minimal_planner"
            }

            fn plan(
                &self,
                _ctx: &ExpressionPlanningContext<'_>,
                plan: &mut ExpressionPlan,
            ) -> anyhow::Result<()> {
                plan.intent = "plugin_adjusted".to_string();
                plan.timeline.clear();
                Ok(())
            }
        }

        impl ExpressionSafetyPolicyPlugin for CapIntensityPolicy {
            fn name(&self) -> &str {
                "test_intensity_cap"
            }

            fn apply(
                &self,
                _ctx: &ExpressionPlanningContext<'_>,
                plan: &mut ExpressionPlan,
            ) -> anyhow::Result<Vec<super::super::types::ExpressionSafetyModification>>
            {
                let before = serde_json::json!(plan.intensity);
                plan.intensity = plan.intensity.min(0.2);
                if let Some(speech) = &mut plan.speech {
                    speech.intensity = speech.intensity.min(0.2);
                }
                Ok(vec![super::super::extension::safety_modification(
                    self.name(),
                    "intensity",
                    "test cap",
                    Some(before),
                    Some(serde_json::json!(plan.intensity)),
                )])
            }
        }

        let mut styles = BTreeMap::new();
        styles.insert("cheerful".to_string(), ExpressionStyleSection::default());
        styles.insert("neutral".to_string(), ExpressionStyleSection::default());

        let runtime = ExpressionRuntime {
            policy: ExpressionPolicySection::default(),
            regulation: RegulationPolicySection::default(),
            affect: AffectStore::new(AffectPolicySection::default()),
            styles,
            dialogue_policy: DialoguePolicyResourceSection::default(),
            extensions: ExpressionExtensionRegistry::default(),
            safety_state: ExpressionSafetyRuntimeState::default(),
        };
        runtime
            .register_expression_planner(Arc::new(MinimalPlanner))
            .unwrap();
        runtime
            .register_expression_safety_policy(Arc::new(CapIntensityPolicy))
            .unwrap();

        let plan = runtime
            .plan(ExpressionInput {
                response_text: "Great, awesome!".to_string(),
                route: Some("minor_llm".to_string()),
                channel_id: "cli".to_string(),
                user_id: "user".to_string(),
                dialogue_act: "respond".to_string(),
                content_tone: "neutral".to_string(),
                user_state: Default::default(),
                device_context: Default::default(),
                safety_context: Default::default(),
                conversation_context: Default::default(),
                constraints: Default::default(),
                voice_output: true,
                hardware_control: true,
                tts_capabilities: None,
                expression_capabilities: None,
            })
            .await;

        assert_eq!(plan.intent, "plugin_adjusted");
        assert!(plan.timeline.is_empty());
        assert!(plan.intensity <= 0.2);
        assert_eq!(plan.safety_modifications.len(), 1);
    }

    fn capabilities_with_action(action: &str, max_intensity: f32) -> ExpressionCapabilitySnapshot {
        let mut capabilities = BTreeMap::new();
        capabilities.insert(
            action.to_string(),
            ExpressionActionCapability {
                supported: true,
                max_intensity: Some(max_intensity),
                max_duration_ms: Some(900),
                ..ExpressionActionCapability::default()
            },
        );
        ExpressionCapabilitySnapshot {
            adapter: "test_adapter".to_string(),
            capabilities,
            current_state: ExpressionAdapterState {
                motion_adapter_registered: true,
                ..ExpressionAdapterState::default()
            },
        }
    }

    fn test_expression_input(text: &str) -> ExpressionInput {
        ExpressionInput {
            response_text: text.to_string(),
            route: Some("minor_llm".to_string()),
            channel_id: "cli".to_string(),
            user_id: "user".to_string(),
            dialogue_act: "respond".to_string(),
            content_tone: "neutral".to_string(),
            user_state: Default::default(),
            device_context: Default::default(),
            safety_context: Default::default(),
            conversation_context: Default::default(),
            constraints: Default::default(),
            voice_output: true,
            hardware_control: false,
            tts_capabilities: None,
            expression_capabilities: None,
        }
    }
}
