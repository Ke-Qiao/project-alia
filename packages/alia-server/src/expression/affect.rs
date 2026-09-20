use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::AffectPolicySection;

use super::types::{
    AffectAxes, AffectSnapshot, RelationshipPrivacyControls, RelationshipProfile, ResourceState,
    SocialState,
};

const RELATIONSHIP_DELTA_SCALE: f32 = 0.2;
const RELATIONSHIP_HOMEOSTASIS_SCALE: f32 = 0.15;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffectObservation {
    pub snapshot: AffectSnapshot,
    pub appraisals: Vec<AppraisalEvent>,
    pub active_event_count: usize,
    /// Legacy source-free APIs return an admission error and no appraisals.
    /// Canonical writes return AppraisalReceipt after the database commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence_error: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct AffectUserInput<'a> {
    pub channel_id: &'a str,
    pub user_id: &'a str,
    pub user_text: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct AffectAssistantResponse<'a> {
    pub channel_id: &'a str,
    pub user_id: &'a str,
    pub response_text: &'a str,
    pub route: &'a str,
    pub used_tools: bool,
    pub response_was_fallback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppraisalEvent {
    pub id: Uuid,
    pub kind: String,
    pub source: AppraisalSource,
    pub target: AppraisalTarget,
    pub confidence: f32,
    pub scope: AppraisalScope,
    pub created_at: DateTime<Utc>,
    pub ttl_sec: u64,
    pub decay: AppraisalDecay,
    pub deltas: AffectDeltas,
    pub evidence: AppraisalEvidence,
}

impl AppraisalEvent {
    pub(crate) fn new(
        kind: impl Into<String>,
        source: AppraisalSource,
        target: AppraisalTarget,
        confidence: f32,
        scope: AppraisalScope,
        now: DateTime<Utc>,
        policy: &AffectPolicySection,
        deltas: AffectDeltas,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: kind.into(),
            source,
            target,
            confidence: confidence.clamp(0.0, 1.0),
            scope,
            created_at: now,
            ttl_sec: ttl_for_scope(scope, policy),
            decay: AppraisalDecay {
                kind: decay_for_scope(scope),
                half_life_sec: half_life_for_scope(scope, policy),
            },
            deltas,
            evidence: AppraisalEvidence {
                summary: summary.into(),
            },
        }
    }

    pub(crate) fn is_active(&self, now: DateTime<Utc>) -> bool {
        let Some(ttl) = i64::try_from(self.ttl_sec)
            .ok()
            .and_then(chrono::Duration::try_seconds)
        else {
            return false;
        };
        self.created_at <= now
            && self
                .created_at
                .checked_add_signed(ttl)
                .is_some_and(|expires| now < expires)
    }

    fn weight(&self, now: DateTime<Utc>) -> f32 {
        if !self.is_active(now) {
            return 0.0;
        }
        let age_sec = event_age_sec(self.created_at, now) as f32;
        let ttl_sec = self.ttl_sec.max(1) as f32;
        let decay_weight = match self.decay.kind {
            AppraisalDecayKind::Exponential => {
                let half_life = self.decay.half_life_sec.max(1) as f32;
                0.5_f32.powf(age_sec / half_life)
            }
            AppraisalDecayKind::Linear => (1.0 - age_sec / ttl_sec).clamp(0.0, 1.0),
            AppraisalDecayKind::Step => 1.0,
            AppraisalDecayKind::Sticky => {
                let half_life = (self.decay.half_life_sec.max(1) as f32) * 4.0;
                0.5_f32.powf(age_sec / half_life).max(0.25)
            }
        };
        (self.confidence * decay_weight).clamp(0.0, 1.0)
    }

    fn weighted_strength(&self, now: DateTime<Utc>) -> f32 {
        self.weight(now) * self.deltas.abs_sum()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppraisalSource {
    UserMessage,
    AssistantResponse,
    Classifier,
    Llm,
    Runtime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppraisalTarget {
    Assistant,
    User,
    Dialogue,
    Runtime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppraisalScope {
    Turn,
    Session,
    User,
    Relationship,
    Device,
    Global,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppraisalDecayKind {
    Exponential,
    Linear,
    Step,
    Sticky,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AppraisalDecay {
    pub kind: AppraisalDecayKind,
    pub half_life_sec: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct AffectDeltas {
    pub valence: f32,
    pub arousal: f32,
    pub calmness: f32,
    pub engagement: f32,
    pub fatigue: f32,
    pub battery_stress: f32,
    pub thermal_stress: f32,
    pub sleepiness: f32,
    pub interaction_load: f32,
    pub trust: f32,
    pub familiarity: f32,
}

impl AffectDeltas {
    fn abs_sum(&self) -> f32 {
        self.valence.abs()
            + self.arousal.abs()
            + self.calmness.abs()
            + self.engagement.abs()
            + self.fatigue.abs()
            + self.battery_stress.abs()
            + self.thermal_stress.abs()
            + self.sleepiness.abs()
            + self.interaction_load.abs()
            + self.trust.abs()
            + self.familiarity.abs()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppraisalEvidence {
    pub summary: String,
}

#[derive(Debug, Clone, Default)]
struct AffectState {
    events: Vec<AppraisalEvent>,
}

/// Replay only the already validated, bounded set. Neither old profile text nor
/// a running interpreter participates in rebuilding this projection.
pub(crate) fn reduce_validated_events(
    owner: Uuid,
    events: Vec<AppraisalEvent>,
    policy: &AffectPolicySection,
    as_of: DateTime<Utc>,
) -> (AffectSnapshot, RelationshipProfile) {
    let state = AffectState { events };
    let snapshot = if policy.enabled {
        state.snapshot(policy, as_of)
    } else {
        baseline_snapshot(policy)
    };
    let relationships = state
        .events
        .iter()
        .filter(|event| {
            event.is_active(as_of)
                && matches!(
                    event.scope,
                    AppraisalScope::User | AppraisalScope::Relationship
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    let social = if policy.relationship_profile_enabled {
        AffectState {
            events: relationships.clone(),
        }
        .social_state(policy, as_of)
    } else {
        baseline_social_state(policy)
    };
    let mut preferences = BTreeMap::new();
    if policy.relationship_profile_store_preferences {
        for event in &relationships {
            if event.kind.contains("preference") {
                // Evidence is a normalized reason code, never private source text.
                preferences.insert(event.kind.clone(), event.kind.clone());
            }
        }
        prune_preferences(&mut preferences, policy.relationship_preferences_max_items);
    }
    let signals =
        relationship_signal_counts(&relationships, policy.relationship_summary_max_signals);
    let profile = RelationshipProfile {
        relationship_epoch: 0,
        user_id: owner.to_string(),
        trust: social.trust,
        familiarity: social.familiarity,
        summary: relationship_summary(social, &signals, preferences.len()),
        preference_summary: preference_summary(&preferences),
        preferences,
        signal_counts: signals,
        privacy: RelationshipPrivacyControls {
            user_visible: policy.relationship_profile_user_visible,
            stores_preferences: policy.relationship_profile_store_preferences,
            stores_raw_evidence: false,
            clearable: policy.relationship_profile_user_clearable,
        },
        updated_at: as_of.to_rfc3339(),
    };
    (snapshot, profile)
}

impl AffectState {
    fn snapshot(&self, policy: &AffectPolicySection, now: DateTime<Utc>) -> AffectSnapshot {
        let baseline_affect = baseline_affect_state(policy);
        let baseline_resource = baseline_resource_state(policy);
        let baseline_social = baseline_social_state(policy);
        let mut affect = baseline_affect;
        let mut resource = baseline_resource;
        let mut social = baseline_social;
        let mut active_events = self
            .events
            .iter()
            .filter(|event| event.is_active(now))
            .collect::<Vec<_>>();
        active_events.sort_by_key(|event| (event.created_at, event.id));

        let mut dominant = Vec::new();
        for event in active_events {
            let weight = event.weight(now);
            affect = apply_affect_event_delta(affect, event.deltas, weight, policy);
            resource = apply_resource_event_delta(resource, event.deltas, weight, policy);
            social = apply_social_event_delta(social, event.deltas, weight, policy);
            let strength = event.weighted_strength(now);
            if strength > 0.001 {
                dominant.push((event.kind.clone(), strength));
            }
        }

        dominant.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let dominant_causes = dominant
            .into_iter()
            .take(4)
            .map(|(kind, _)| kind)
            .collect::<Vec<_>>();

        AffectSnapshot {
            affect: apply_affect_homeostasis(baseline_affect, affect, policy),
            social: apply_social_homeostasis(baseline_social, social, policy),
            resource: apply_resource_homeostasis(baseline_resource, resource, policy),
            dominant_causes: if dominant_causes.is_empty() {
                vec!["baseline".to_string()]
            } else {
                dominant_causes
            },
        }
    }

    fn social_state(&self, policy: &AffectPolicySection, now: DateTime<Utc>) -> SocialState {
        let baseline = baseline_social_state(policy);
        let mut social = baseline;
        let mut active_events = self
            .events
            .iter()
            .filter(|event| event.is_active(now))
            .collect::<Vec<_>>();
        active_events.sort_by_key(|event| (event.created_at, event.id));
        for event in active_events {
            let weight = event.weight(now);
            social = apply_social_event_delta(social, event.deltas, weight, policy);
        }
        apply_social_homeostasis(baseline, social, policy)
    }
}

pub(crate) fn appraise_assistant_response(
    input: AffectAssistantResponse<'_>,
    now: DateTime<Utc>,
    policy: &AffectPolicySection,
) -> Vec<AppraisalEvent> {
    let lower = input.response_text.to_lowercase();
    let mut events = Vec::new();

    if policy.fatigue_per_turn > 0.0 {
        events.push(appraisal(
            "turn_fatigue",
            AppraisalSource::Runtime,
            AppraisalTarget::Runtime,
            1.0,
            AppraisalScope::Session,
            now,
            policy,
            AffectDeltas {
                fatigue: policy.fatigue_per_turn.clamp(0.0, 1.0),
                interaction_load: (policy.fatigue_per_turn * 0.5).clamp(0.0, 1.0),
                ..AffectDeltas::default()
            },
            "One assistant turn completed",
        ));
    }
    if input.response_text.chars().count() > 300 {
        events.push(appraisal(
            "long_response",
            AppraisalSource::AssistantResponse,
            AppraisalTarget::Runtime,
            0.92,
            AppraisalScope::Session,
            now,
            policy,
            AffectDeltas {
                fatigue: policy.long_response_fatigue_delta.clamp(0.0, 1.0),
                interaction_load: (policy.long_response_fatigue_delta * 0.75).clamp(0.0, 1.0),
                ..AffectDeltas::default()
            },
            "Assistant produced a long response",
        ));
    }
    if input.response_was_fallback {
        events.push(appraisal(
            "fallback_response",
            AppraisalSource::Runtime,
            AppraisalTarget::Dialogue,
            0.95,
            AppraisalScope::Session,
            now,
            policy,
            AffectDeltas {
                valence: -0.08,
                arousal: 0.05,
                calmness: -0.04,
                engagement: -0.04,
                ..AffectDeltas::default()
            },
            "Pipeline used fallback text",
        ));
    }
    if input.used_tools {
        events.push(appraisal(
            "tool_assisted_turn",
            AppraisalSource::Runtime,
            AppraisalTarget::Dialogue,
            0.72,
            AppraisalScope::Turn,
            now,
            policy,
            AffectDeltas {
                engagement: 0.03,
                ..AffectDeltas::default()
            },
            "Turn used one or more tools",
        ));
    }
    if input.route.contains("cache") {
        events.push(appraisal(
            "fast_cached_turn",
            AppraisalSource::Runtime,
            AppraisalTarget::Dialogue,
            0.70,
            AppraisalScope::Turn,
            now,
            policy,
            AffectDeltas {
                arousal: -0.02,
                calmness: 0.03,
                ..AffectDeltas::default()
            },
            "Turn was served from cache",
        ));
    }
    if contains_any(&lower, &["抱歉", "对不起", "sorry", "apologize"]) {
        events.push(appraisal(
            "assistant_apology",
            AppraisalSource::AssistantResponse,
            AppraisalTarget::Dialogue,
            0.82,
            AppraisalScope::Turn,
            now,
            policy,
            AffectDeltas {
                valence: -0.03,
                arousal: -0.03,
                calmness: 0.02,
                engagement: 0.02,
                ..AffectDeltas::default()
            },
            "Assistant apologized or acknowledged a correction",
        ));
    }
    if contains_any(&lower, &["太好了", "真棒", "开心", "great", "awesome"]) {
        events.push(appraisal(
            "positive_assistant_tone",
            AppraisalSource::AssistantResponse,
            AppraisalTarget::Dialogue,
            0.70,
            AppraisalScope::Turn,
            now,
            policy,
            AffectDeltas {
                valence: 0.06,
                arousal: 0.04,
                ..AffectDeltas::default()
            },
            "Assistant response used a positive tone",
        ));
    }

    filter_appraisals(events, policy)
}

fn appraisal(
    kind: &str,
    source: AppraisalSource,
    target: AppraisalTarget,
    confidence: f32,
    scope: AppraisalScope,
    now: DateTime<Utc>,
    policy: &AffectPolicySection,
    deltas: AffectDeltas,
    summary: &str,
) -> AppraisalEvent {
    AppraisalEvent::new(
        kind, source, target, confidence, scope, now, policy, deltas, summary,
    )
}

pub(crate) fn filter_appraisals(
    events: Vec<AppraisalEvent>,
    policy: &AffectPolicySection,
) -> Vec<AppraisalEvent> {
    events
        .into_iter()
        .filter(|event| {
            event.confidence >= policy.min_appraisal_confidence.clamp(0.0, 1.0)
                && event.deltas.abs_sum() > 0.0
        })
        .collect()
}

pub(crate) fn ttl_for_scope(scope: AppraisalScope, policy: &AffectPolicySection) -> u64 {
    match scope {
        AppraisalScope::Turn => policy.turn_appraisal_ttl_sec.max(1),
        AppraisalScope::Session => policy.session_appraisal_ttl_sec.max(1),
        AppraisalScope::User
        | AppraisalScope::Relationship
        | AppraisalScope::Device
        | AppraisalScope::Global => policy.relationship_appraisal_ttl_sec.max(1),
    }
}

pub(crate) fn half_life_for_scope(scope: AppraisalScope, policy: &AffectPolicySection) -> u64 {
    match scope {
        AppraisalScope::User
        | AppraisalScope::Relationship
        | AppraisalScope::Device
        | AppraisalScope::Global => policy.relationship_appraisal_half_life_sec.max(1),
        AppraisalScope::Turn | AppraisalScope::Session => policy.appraisal_half_life_sec.max(1),
    }
}

fn decay_for_scope(scope: AppraisalScope) -> AppraisalDecayKind {
    match scope {
        AppraisalScope::User | AppraisalScope::Relationship | AppraisalScope::Global => {
            AppraisalDecayKind::Sticky
        }
        AppraisalScope::Turn | AppraisalScope::Session | AppraisalScope::Device => {
            AppraisalDecayKind::Exponential
        }
    }
}

fn event_age_sec(created_at: DateTime<Utc>, now: DateTime<Utc>) -> u64 {
    now.signed_duration_since(created_at).num_seconds().max(0) as u64
}

pub(crate) fn baseline_snapshot(policy: &AffectPolicySection) -> AffectSnapshot {
    AffectSnapshot {
        affect: baseline_affect_state(policy),
        social: baseline_social_state(policy),
        resource: baseline_resource_state(policy),
        dominant_causes: vec!["baseline".to_string()],
    }
}

fn baseline_affect_state(policy: &AffectPolicySection) -> AffectAxes {
    AffectAxes {
        valence: policy.baseline_valence.clamp(-1.0, 1.0),
        arousal: policy.baseline_arousal.clamp(0.0, 1.0),
        calmness: policy.baseline_calmness.clamp(0.0, 1.0),
    }
}

fn baseline_social_state(policy: &AffectPolicySection) -> SocialState {
    SocialState {
        trust: policy.baseline_trust.clamp(0.0, 1.0),
        familiarity: policy.baseline_familiarity.clamp(0.0, 1.0),
        engagement: policy.baseline_engagement.clamp(0.0, 1.0),
    }
}

fn baseline_resource_state(policy: &AffectPolicySection) -> ResourceState {
    ResourceState {
        fatigue: policy.baseline_fatigue.clamp(0.0, 1.0),
        battery_stress: policy.baseline_battery_stress.clamp(0.0, 1.0),
        thermal_stress: policy.baseline_thermal_stress.clamp(0.0, 1.0),
        sleepiness: policy.baseline_sleepiness.clamp(0.0, 1.0),
        interaction_load: policy.baseline_interaction_load.clamp(0.0, 1.0),
    }
}

fn apply_affect_event_delta(
    current: AffectAxes,
    deltas: AffectDeltas,
    weight: f32,
    policy: &AffectPolicySection,
) -> AffectAxes {
    let cap = policy.max_delta_per_turn.max(0.0);
    AffectAxes {
        valence: reduce_axis_step(current.valence, deltas.valence * weight, -1.0, 1.0, cap),
        arousal: reduce_axis_step(current.arousal, deltas.arousal * weight, 0.0, 1.0, cap),
        calmness: reduce_axis_step(current.calmness, deltas.calmness * weight, 0.0, 1.0, cap),
    }
}

fn apply_resource_event_delta(
    current: ResourceState,
    deltas: AffectDeltas,
    weight: f32,
    policy: &AffectPolicySection,
) -> ResourceState {
    ResourceState {
        fatigue: reduce_axis_step(
            current.fatigue,
            deltas.fatigue * weight,
            0.0,
            1.0,
            policy.max_delta_per_turn.max(0.0),
        ),
        battery_stress: reduce_axis_step(
            current.battery_stress,
            deltas.battery_stress * weight,
            0.0,
            1.0,
            policy.max_delta_per_turn.max(0.0),
        ),
        thermal_stress: reduce_axis_step(
            current.thermal_stress,
            deltas.thermal_stress * weight,
            0.0,
            1.0,
            policy.max_delta_per_turn.max(0.0),
        ),
        sleepiness: reduce_axis_step(
            current.sleepiness,
            deltas.sleepiness * weight,
            0.0,
            1.0,
            policy.max_delta_per_turn.max(0.0),
        ),
        interaction_load: reduce_axis_step(
            current.interaction_load,
            deltas.interaction_load * weight,
            0.0,
            1.0,
            policy.max_delta_per_turn.max(0.0),
        ),
    }
}

fn apply_social_event_delta(
    current: SocialState,
    deltas: AffectDeltas,
    weight: f32,
    policy: &AffectPolicySection,
) -> SocialState {
    let relationship_cap = policy.max_delta_per_turn.max(0.0) * RELATIONSHIP_DELTA_SCALE;
    SocialState {
        trust: reduce_axis_step(
            current.trust,
            deltas.trust * weight,
            0.0,
            1.0,
            relationship_cap,
        ),
        familiarity: reduce_axis_step(
            current.familiarity,
            deltas.familiarity * weight,
            0.0,
            1.0,
            relationship_cap,
        ),
        engagement: reduce_axis_step(
            current.engagement,
            deltas.engagement * weight,
            0.0,
            1.0,
            policy.max_delta_per_turn.max(0.0),
        ),
    }
}

fn apply_affect_homeostasis(
    baseline: AffectAxes,
    current: AffectAxes,
    policy: &AffectPolicySection,
) -> AffectAxes {
    AffectAxes {
        valence: apply_homeostasis_axis(
            baseline.valence,
            current.valence,
            -1.0,
            1.0,
            policy.homeostasis_rate,
        ),
        arousal: apply_homeostasis_axis(
            baseline.arousal,
            current.arousal,
            0.0,
            1.0,
            policy.homeostasis_rate,
        ),
        calmness: apply_homeostasis_axis(
            baseline.calmness,
            current.calmness,
            0.0,
            1.0,
            policy.homeostasis_rate,
        ),
    }
}

fn apply_resource_homeostasis(
    baseline: ResourceState,
    current: ResourceState,
    policy: &AffectPolicySection,
) -> ResourceState {
    ResourceState {
        fatigue: apply_homeostasis_axis(
            baseline.fatigue,
            current.fatigue,
            0.0,
            1.0,
            policy.homeostasis_rate,
        ),
        battery_stress: apply_homeostasis_axis(
            baseline.battery_stress,
            current.battery_stress,
            0.0,
            1.0,
            policy.homeostasis_rate,
        ),
        thermal_stress: apply_homeostasis_axis(
            baseline.thermal_stress,
            current.thermal_stress,
            0.0,
            1.0,
            policy.homeostasis_rate,
        ),
        sleepiness: apply_homeostasis_axis(
            baseline.sleepiness,
            current.sleepiness,
            0.0,
            1.0,
            policy.homeostasis_rate,
        ),
        interaction_load: apply_homeostasis_axis(
            baseline.interaction_load,
            current.interaction_load,
            0.0,
            1.0,
            policy.homeostasis_rate,
        ),
    }
}

fn apply_social_homeostasis(
    baseline: SocialState,
    current: SocialState,
    policy: &AffectPolicySection,
) -> SocialState {
    let relationship_homeostasis = policy.homeostasis_rate * RELATIONSHIP_HOMEOSTASIS_SCALE;
    SocialState {
        trust: apply_homeostasis_axis(
            baseline.trust,
            current.trust,
            0.0,
            1.0,
            relationship_homeostasis,
        ),
        familiarity: apply_homeostasis_axis(
            baseline.familiarity,
            current.familiarity,
            0.0,
            1.0,
            relationship_homeostasis,
        ),
        engagement: apply_homeostasis_axis(
            baseline.engagement,
            current.engagement,
            0.0,
            1.0,
            policy.homeostasis_rate,
        ),
    }
}

fn reduce_axis_step(current: f32, delta: f32, min: f32, max: f32, max_delta: f32) -> f32 {
    let limited_delta = delta.clamp(-max_delta.abs(), max_delta.abs());
    saturating_axis_add(current.clamp(min, max), limited_delta, min, max)
}

fn apply_homeostasis_axis(baseline: f32, current: f32, min: f32, max: f32, rate: f32) -> f32 {
    let rate = rate.clamp(0.0, 1.0);
    (baseline + (current - baseline) * (1.0 - rate)).clamp(min, max)
}

fn saturating_axis_add(current: f32, delta: f32, min: f32, max: f32) -> f32 {
    if delta.abs() <= f32::EPSILON {
        return current.clamp(min, max);
    }
    let distance_to_bound = if delta > 0.0 {
        max - current
    } else {
        current - min
    };
    let saturation = distance_to_bound.clamp(0.0, 1.0);
    (current + delta * saturation).clamp(min, max)
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn prune_preferences(preferences: &mut BTreeMap<String, String>, max_items: usize) {
    if max_items == 0 {
        preferences.clear();
        return;
    }
    while preferences.len() > max_items {
        let Some(first_key) = preferences.keys().next().cloned() else {
            break;
        };
        preferences.remove(&first_key);
    }
}

fn relationship_signal_counts(
    events: &[AppraisalEvent],
    max_signals: usize,
) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for event in events {
        *counts.entry(event.kind.clone()).or_insert(0) += 1;
    }
    while max_signals > 0 && counts.len() > max_signals {
        let Some(first_key) = counts.keys().next().cloned() else {
            break;
        };
        counts.remove(&first_key);
    }
    if max_signals == 0 {
        counts.clear();
    }
    counts
}

fn preference_summary(preferences: &BTreeMap<String, String>) -> String {
    if preferences.is_empty() {
        "No durable interaction preferences recorded.".to_string()
    } else {
        format!(
            "{} durable preference signal(s) recorded.",
            preferences.len()
        )
    }
}

fn relationship_summary(
    social: SocialState,
    signal_counts: &BTreeMap<String, usize>,
    preference_count: usize,
) -> String {
    let trust_band = if social.trust >= 0.72 {
        "high trust"
    } else if social.trust >= 0.45 {
        "developing trust"
    } else {
        "careful repair"
    };
    let familiarity_band = if social.familiarity >= 0.55 {
        "familiar"
    } else if social.familiarity >= 0.22 {
        "warming up"
    } else {
        "new relationship"
    };
    let signals = if signal_counts.is_empty() {
        "no recent relationship signals".to_string()
    } else {
        signal_counts
            .iter()
            .map(|(kind, count)| format!("{kind}:{count}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "{trust_band}; {familiarity_band}; preferences={preference_count}; recent_signals={signals}"
    )
}

#[cfg(test)]
mod tests;
