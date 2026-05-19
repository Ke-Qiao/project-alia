use std::collections::BTreeMap;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::AffectPolicySection;
use crate::entity::expression_appraisal_event;

use super::types::{
    AffectAxes, AffectSnapshot, RelationshipClearResult, RelationshipPrivacyControls,
    RelationshipProfile, RelationshipPruneResult, ResourceState, SocialState,
};

const RELATIONSHIP_DELTA_SCALE: f32 = 0.2;
const RELATIONSHIP_HOMEOSTASIS_SCALE: f32 = 0.15;

#[derive(Debug)]
pub struct AffectStore {
    policy: AffectPolicySection,
    states: RwLock<BTreeMap<AffectKey, AffectState>>,
    relationships: RwLock<BTreeMap<String, AffectState>>,
    relationship_profiles: RwLock<BTreeMap<String, RelationshipProfile>>,
    devices: RwLock<BTreeMap<String, AffectState>>,
    global: RwLock<AffectState>,
    db: RwLock<Option<DatabaseConnection>>,
}

impl AffectStore {
    pub fn new(policy: AffectPolicySection) -> Self {
        Self {
            policy,
            states: RwLock::new(BTreeMap::new()),
            relationships: RwLock::new(BTreeMap::new()),
            relationship_profiles: RwLock::new(BTreeMap::new()),
            devices: RwLock::new(BTreeMap::new()),
            global: RwLock::new(AffectState::default()),
            db: RwLock::new(None),
        }
    }

    pub async fn attach_db(&self, db: DatabaseConnection) -> anyhow::Result<()> {
        if self.policy.prune_persisted_appraisals_on_startup {
            let pruned = prune_persisted_appraisals(&db, &self.policy).await?;
            if pruned > 0 {
                tracing::info!(
                    pruned_count = pruned,
                    "pruned expired expression appraisal events from database"
                );
            }
        }
        let restored = load_active_appraisals(&db, &self.policy).await?;
        self.restore_appraisals(restored);
        let mut db_slot = self
            .db
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *db_slot = Some(db);
        Ok(())
    }

    pub fn snapshot(&self, channel_id: &str, user_id: &str) -> AffectSnapshot {
        if !self.policy.enabled {
            return baseline_snapshot(&self.policy);
        }

        let now = Utc::now();
        let mut snapshot = {
            let states = self
                .states
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            states
                .get(&AffectKey::new(channel_id, user_id))
                .map(|state| state.snapshot(&self.policy, now))
                .unwrap_or_else(|| baseline_snapshot(&self.policy))
        };
        let relationship = self.relationship_snapshot(user_id, now);
        snapshot.social.trust = relationship.social.trust;
        snapshot.social.familiarity = relationship.social.familiarity;
        snapshot = merge_context_snapshot(snapshot, relationship, &self.policy);
        snapshot = self.merge_global_snapshot(snapshot, now);
        snapshot = self.merge_device_snapshot(snapshot, channel_id, now);
        snapshot
    }

    pub(crate) fn policy(&self) -> &AffectPolicySection {
        &self.policy
    }

    pub fn relationship_profile(&self, user_id: &str) -> RelationshipProfile {
        self.relationship_profiles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(user_id)
            .cloned()
            .unwrap_or_else(|| {
                self.build_relationship_profile(
                    user_id,
                    Utc::now(),
                    baseline_social_state(&self.policy),
                    &[],
                )
            })
    }

    pub async fn clear_relationship_data(
        &self,
        user_id: &str,
    ) -> anyhow::Result<RelationshipClearResult> {
        let relationship_events_removed = self
            .relationships
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(user_id)
            .map(|state| state.events.len())
            .unwrap_or(0);
        let profile_removed = self
            .relationship_profiles
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(user_id)
            .is_some();
        let db = self
            .db
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let persisted_events_removed = if let Some(db) = db {
            delete_persisted_relationship_appraisals(&db, user_id).await?
        } else {
            0
        };

        Ok(RelationshipClearResult {
            user_id: user_id.to_string(),
            profile_removed,
            relationship_events_removed,
            persisted_events_removed,
        })
    }

    pub fn prune_relationship_profiles(&self) -> RelationshipPruneResult {
        let now = Utc::now();
        let prune_after = self.policy.relationship_profile_prune_after_sec;
        if prune_after == 0 {
            return RelationshipPruneResult {
                profiles_removed: 0,
                relationship_states_removed: 0,
            };
        }
        let max_age = chrono::Duration::seconds(prune_after.min(i64::MAX as u64) as i64);
        let stale_users = self
            .relationship_profiles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter_map(|(user_id, profile)| {
                DateTime::parse_from_rfc3339(&profile.updated_at)
                    .ok()
                    .map(|updated| updated.with_timezone(&Utc))
                    .filter(|updated| now.signed_duration_since(*updated) > max_age)
                    .map(|_| user_id.clone())
            })
            .collect::<Vec<_>>();

        let profiles_removed = {
            let mut profiles = self
                .relationship_profiles
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            stale_users
                .iter()
                .filter(|user_id| profiles.remove(*user_id).is_some())
                .count()
        };
        let relationship_states_removed = {
            let mut relationships = self
                .relationships
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            stale_users
                .iter()
                .filter(|user_id| relationships.remove(*user_id).is_some())
                .count()
        };

        RelationshipPruneResult {
            profiles_removed,
            relationship_states_removed,
        }
    }

    pub fn observe_user_appraisals(
        &self,
        input: AffectUserInput<'_>,
        appraisals: Vec<AppraisalEvent>,
    ) -> AffectObservation {
        if !self.policy.enabled {
            return AffectObservation::baseline(&self.policy);
        }

        let now = Utc::now();
        self.apply_appraisals(input.channel_id, input.user_id, None, now, appraisals)
    }

    pub fn observe_assistant_response(
        &self,
        input: AffectAssistantResponse<'_>,
    ) -> AffectObservation {
        if !self.policy.enabled {
            return AffectObservation::baseline(&self.policy);
        }

        let now = Utc::now();
        let appraisals = appraise_assistant_response(input, now, &self.policy);
        self.apply_appraisals(
            input.channel_id,
            input.user_id,
            Some(input.route),
            now,
            appraisals,
        )
    }

    fn apply_appraisals(
        &self,
        channel_id: &str,
        user_id: &str,
        route: Option<&str>,
        now: DateTime<Utc>,
        appraisals: Vec<AppraisalEvent>,
    ) -> AffectObservation {
        let mut session_appraisals = Vec::new();
        let mut relationship_appraisals = Vec::new();
        let mut device_appraisals = Vec::new();
        let mut global_appraisals = Vec::new();
        for event in appraisals.iter().cloned() {
            match event.scope {
                AppraisalScope::Relationship | AppraisalScope::User => {
                    relationship_appraisals.push(event)
                }
                AppraisalScope::Device => device_appraisals.push(event),
                AppraisalScope::Global => global_appraisals.push(event),
                AppraisalScope::Turn | AppraisalScope::Session => session_appraisals.push(event),
            }
        }

        let (mut snapshot, session_event_count) = {
            let mut states = self
                .states
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let state = states
                .entry(AffectKey::new(channel_id, user_id))
                .or_insert_with(AffectState::default);
            state.prune_expired(now);
            state.append_appraisals(session_appraisals, now, &self.policy);
            (
                state.snapshot(&self.policy, now),
                state.active_event_count(now),
            )
        };

        let relationship_event_count =
            self.apply_relationship_appraisals(user_id, now, relationship_appraisals);
        let device_event_count = self.apply_device_appraisals(channel_id, now, device_appraisals);
        let global_event_count = self.apply_global_appraisals(now, global_appraisals);
        let relationship = self.relationship_snapshot(user_id, now);
        snapshot.social.trust = relationship.social.trust;
        snapshot.social.familiarity = relationship.social.familiarity;
        snapshot = merge_context_snapshot(snapshot, relationship, &self.policy);
        snapshot = self.merge_global_snapshot(snapshot, now);
        snapshot = self.merge_device_snapshot(snapshot, channel_id, now);
        self.persist_appraisals(channel_id, user_id, route, &appraisals);

        AffectObservation {
            snapshot,
            appraisals,
            active_event_count: session_event_count
                + relationship_event_count
                + device_event_count
                + global_event_count,
        }
    }

    fn apply_relationship_appraisals(
        &self,
        user_id: &str,
        now: DateTime<Utc>,
        appraisals: Vec<AppraisalEvent>,
    ) -> usize {
        let mut relationships = self
            .relationships
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let state = relationships
            .entry(user_id.to_string())
            .or_insert_with(AffectState::default);
        state.prune_expired(now);
        state.append_appraisals(appraisals.clone(), now, &self.policy);
        let social = state.social_state(&self.policy, now);
        if self.policy.relationship_profile_enabled {
            let profile = self.build_relationship_profile(user_id, now, social, &appraisals);
            self.relationship_profiles
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(user_id.to_string(), profile);
        }
        state.active_event_count(now)
    }

    fn build_relationship_profile(
        &self,
        user_id: &str,
        now: DateTime<Utc>,
        social: SocialState,
        new_events: &[AppraisalEvent],
    ) -> RelationshipProfile {
        let mut preferences = self
            .relationship_profiles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(user_id)
            .map(|profile| profile.preferences.clone())
            .unwrap_or_default();
        if self.policy.relationship_profile_store_preferences {
            for event in new_events
                .iter()
                .filter(|event| event.kind.contains("preference"))
            {
                preferences.insert(event.kind.clone(), event.evidence.summary.clone());
            }
            prune_preferences(
                &mut preferences,
                self.policy.relationship_preferences_max_items,
            );
        } else {
            preferences.clear();
        }
        let signal_counts =
            relationship_signal_counts(new_events, self.policy.relationship_summary_max_signals);
        let preference_summary = preference_summary(&preferences);
        RelationshipProfile {
            user_id: user_id.to_string(),
            trust: social.trust,
            familiarity: social.familiarity,
            summary: relationship_summary(social, &signal_counts, preferences.len()),
            preference_summary,
            preferences,
            signal_counts,
            privacy: RelationshipPrivacyControls {
                user_visible: self.policy.relationship_profile_user_visible,
                stores_preferences: self.policy.relationship_profile_store_preferences,
                stores_raw_evidence: self.policy.persist_relationship_evidence,
                clearable: self.policy.relationship_profile_user_clearable,
            },
            updated_at: now.to_rfc3339(),
        }
    }

    fn apply_device_appraisals(
        &self,
        channel_id: &str,
        now: DateTime<Utc>,
        appraisals: Vec<AppraisalEvent>,
    ) -> usize {
        let mut devices = self
            .devices
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let state = devices
            .entry(channel_id.to_string())
            .or_insert_with(AffectState::default);
        state.prune_expired(now);
        state.append_appraisals(appraisals, now, &self.policy);
        state.active_event_count(now)
    }

    fn apply_global_appraisals(
        &self,
        now: DateTime<Utc>,
        appraisals: Vec<AppraisalEvent>,
    ) -> usize {
        let mut global = self
            .global
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        global.prune_expired(now);
        global.append_appraisals(appraisals, now, &self.policy);
        global.active_event_count(now)
    }

    fn relationship_snapshot(&self, user_id: &str, now: DateTime<Utc>) -> AffectSnapshot {
        let relationships = self
            .relationships
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        relationships
            .get(user_id)
            .map(|state| state.snapshot(&self.policy, now))
            .unwrap_or_else(|| baseline_snapshot(&self.policy))
    }

    fn merge_global_snapshot(
        &self,
        snapshot: AffectSnapshot,
        now: DateTime<Utc>,
    ) -> AffectSnapshot {
        let global = self
            .global
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        merge_context_snapshot(snapshot, global.snapshot(&self.policy, now), &self.policy)
    }

    fn merge_device_snapshot(
        &self,
        snapshot: AffectSnapshot,
        channel_id: &str,
        now: DateTime<Utc>,
    ) -> AffectSnapshot {
        let devices = self
            .devices
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        devices
            .get(channel_id)
            .map(|state| {
                merge_context_snapshot(
                    snapshot.clone(),
                    state.snapshot(&self.policy, now),
                    &self.policy,
                )
            })
            .unwrap_or(snapshot)
    }

    fn restore_appraisals(&self, appraisals: Vec<PersistedAppraisal>) {
        if appraisals.is_empty() {
            return;
        }

        let now = Utc::now();
        let mut states = self
            .states
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut relationships = self
            .relationships
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut restored_count = 0usize;
        for persisted in appraisals {
            if !persisted.event.is_active(now) {
                continue;
            }
            restored_count += 1;
            match persisted.event.scope {
                AppraisalScope::Relationship | AppraisalScope::User => {
                    relationships
                        .entry(persisted.user_id)
                        .or_insert_with(AffectState::default)
                        .append_appraisals(vec![persisted.event], now, &self.policy);
                }
                AppraisalScope::Device => {
                    self.devices
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .entry(persisted.channel_id)
                        .or_insert_with(AffectState::default)
                        .append_appraisals(vec![persisted.event], now, &self.policy);
                }
                AppraisalScope::Global => {
                    self.global
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .append_appraisals(vec![persisted.event], now, &self.policy);
                }
                AppraisalScope::Turn | AppraisalScope::Session => {
                    states
                        .entry(AffectKey::new(&persisted.channel_id, &persisted.user_id))
                        .or_insert_with(AffectState::default)
                        .append_appraisals(vec![persisted.event], now, &self.policy);
                }
            }
        }

        tracing::info!(
            restored_count,
            "restored expression appraisal events from database"
        );
    }

    fn persist_appraisals(
        &self,
        channel_id: &str,
        user_id: &str,
        route: Option<&str>,
        appraisals: &[AppraisalEvent],
    ) {
        if appraisals.is_empty() {
            return;
        }

        let db = self
            .db
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(db) = db else {
            return;
        };

        let channel_id = channel_id.to_string();
        let user_id = user_id.to_string();
        let route = route.map(str::to_string);
        let appraisals = appraisals.to_vec();
        let policy = self.policy.clone();

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    if let Err(error) =
                        insert_appraisals(&db, channel_id, user_id, route, appraisals, policy).await
                    {
                        tracing::warn!(
                            error = ?error,
                            "failed to persist expression appraisal events"
                        );
                    }
                });
            }
            Err(error) => {
                tracing::warn!(
                    error = ?error,
                    "no tokio runtime available; skipped expression appraisal persistence"
                );
            }
        }
    }
}

struct PersistedAppraisal {
    channel_id: String,
    user_id: String,
    event: AppraisalEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffectObservation {
    pub snapshot: AffectSnapshot,
    pub appraisals: Vec<AppraisalEvent>,
    pub active_event_count: usize,
}

impl AffectObservation {
    fn baseline(policy: &AffectPolicySection) -> Self {
        Self {
            snapshot: baseline_snapshot(policy),
            appraisals: Vec::new(),
            active_event_count: 0,
        }
    }
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

    fn is_active(&self, now: DateTime<Utc>) -> bool {
        event_age_sec(self.created_at, now) <= self.ttl_sec
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AffectKey {
    channel_id: String,
    user_id: String,
}

impl AffectKey {
    fn new(channel_id: &str, user_id: &str) -> Self {
        Self {
            channel_id: channel_id.to_string(),
            user_id: user_id.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct AffectState {
    events: Vec<AppraisalEvent>,
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
        active_events.sort_by_key(|event| event.created_at);

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
        active_events.sort_by_key(|event| event.created_at);
        for event in active_events {
            let weight = event.weight(now);
            social = apply_social_event_delta(social, event.deltas, weight, policy);
        }
        apply_social_homeostasis(baseline, social, policy)
    }

    fn append_appraisals(
        &mut self,
        appraisals: Vec<AppraisalEvent>,
        now: DateTime<Utc>,
        policy: &AffectPolicySection,
    ) {
        self.events.extend(appraisals);
        self.prune_expired(now);
        self.events.sort_by(|left, right| {
            right
                .weighted_strength(now)
                .partial_cmp(&left.weighted_strength(now))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.created_at.cmp(&left.created_at))
        });
        self.events
            .truncate(policy.max_active_appraisal_events.max(1));
    }

    fn prune_expired(&mut self, now: DateTime<Utc>) {
        self.events.retain(|event| event.is_active(now));
    }

    fn active_event_count(&self, now: DateTime<Utc>) -> usize {
        self.events
            .iter()
            .filter(|event| event.is_active(now))
            .count()
    }
}

fn appraise_assistant_response(
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

fn baseline_snapshot(policy: &AffectPolicySection) -> AffectSnapshot {
    AffectSnapshot {
        affect: baseline_affect_state(policy),
        social: baseline_social_state(policy),
        resource: baseline_resource_state(policy),
        dominant_causes: vec!["baseline".to_string()],
    }
}

fn merge_context_snapshot(
    mut base: AffectSnapshot,
    context: AffectSnapshot,
    policy: &AffectPolicySection,
) -> AffectSnapshot {
    let baseline = baseline_snapshot(policy);
    base.affect.valence =
        (base.affect.valence + context.affect.valence - baseline.affect.valence).clamp(-1.0, 1.0);
    base.affect.arousal =
        (base.affect.arousal + context.affect.arousal - baseline.affect.arousal).clamp(0.0, 1.0);
    base.affect.calmness =
        (base.affect.calmness + context.affect.calmness - baseline.affect.calmness).clamp(0.0, 1.0);
    base.social.engagement = (base.social.engagement + context.social.engagement
        - baseline.social.engagement)
        .clamp(0.0, 1.0);
    base.resource.fatigue = (base.resource.fatigue + context.resource.fatigue
        - baseline.resource.fatigue)
        .clamp(0.0, 1.0);
    base.resource.battery_stress = (base.resource.battery_stress + context.resource.battery_stress
        - baseline.resource.battery_stress)
        .clamp(0.0, 1.0);
    base.resource.thermal_stress = (base.resource.thermal_stress + context.resource.thermal_stress
        - baseline.resource.thermal_stress)
        .clamp(0.0, 1.0);
    base.resource.sleepiness = (base.resource.sleepiness + context.resource.sleepiness
        - baseline.resource.sleepiness)
        .clamp(0.0, 1.0);
    base.resource.interaction_load = (base.resource.interaction_load
        + context.resource.interaction_load
        - baseline.resource.interaction_load)
        .clamp(0.0, 1.0);
    for cause in context.dominant_causes {
        if cause != "baseline" && !base.dominant_causes.contains(&cause) {
            base.dominant_causes.push(cause);
        }
    }
    base
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

async fn load_active_appraisals(
    db: &DatabaseConnection,
    policy: &AffectPolicySection,
) -> anyhow::Result<Vec<PersistedAppraisal>> {
    let threshold = Utc::now()
        - chrono::Duration::seconds(
            max_appraisal_ttl_sec(policy).min(i64::MAX as u64).max(1) as i64
        );
    let rows = expression_appraisal_event::Entity::find()
        .filter(expression_appraisal_event::Column::CreatedAt.gte(threshold))
        .order_by_asc(expression_appraisal_event::Column::CreatedAt)
        .all(db)
        .await?;

    rows.into_iter()
        .map(appraisal_from_model)
        .filter(|result| match result {
            Ok(appraisal) => scope_persistence_allowed(appraisal.event.scope, policy),
            Err(_) => true,
        })
        .collect::<anyhow::Result<Vec<_>>>()
}

async fn prune_persisted_appraisals(
    db: &DatabaseConnection,
    policy: &AffectPolicySection,
) -> anyhow::Result<u64> {
    let threshold = Utc::now()
        - chrono::Duration::seconds(
            max_persisted_retention_sec(policy)
                .min(i64::MAX as u64)
                .max(1) as i64,
        );
    let result = expression_appraisal_event::Entity::delete_many()
        .filter(expression_appraisal_event::Column::CreatedAt.lt(threshold))
        .exec(db)
        .await?;
    Ok(result.rows_affected)
}

async fn delete_persisted_relationship_appraisals(
    db: &DatabaseConnection,
    user_id: &str,
) -> anyhow::Result<u64> {
    let result = expression_appraisal_event::Entity::delete_many()
        .filter(expression_appraisal_event::Column::UserId.eq(user_id))
        .filter(
            expression_appraisal_event::Column::Scope
                .is_in(["relationship".to_string(), "user".to_string()]),
        )
        .exec(db)
        .await?;
    Ok(result.rows_affected)
}

async fn insert_appraisals(
    db: &DatabaseConnection,
    channel_id: String,
    user_id: String,
    route: Option<String>,
    appraisals: Vec<AppraisalEvent>,
    policy: AffectPolicySection,
) -> anyhow::Result<()> {
    if appraisals.is_empty() {
        return Ok(());
    }

    let persistable = appraisals
        .into_iter()
        .filter(|event| scope_persistence_allowed(event.scope, &policy))
        .collect::<Vec<_>>();
    if persistable.is_empty() {
        return Ok(());
    }

    let mut rows = Vec::with_capacity(persistable.len());
    for event in persistable {
        rows.push(appraisal_to_active_model(
            &channel_id,
            &user_id,
            route.as_deref(),
            event,
            &policy,
        )?);
    }

    expression_appraisal_event::Entity::insert_many(rows)
        .exec(db)
        .await?;
    Ok(())
}

fn appraisal_to_active_model(
    channel_id: &str,
    user_id: &str,
    route: Option<&str>,
    event: AppraisalEvent,
    policy: &AffectPolicySection,
) -> anyhow::Result<expression_appraisal_event::ActiveModel> {
    let evidence = evidence_for_persistence(&event, policy)?;
    Ok(expression_appraisal_event::ActiveModel {
        id: Set(event.id),
        channel_id: Set(channel_id.to_string()),
        user_id: Set(user_id.to_string()),
        route: Set(route.map(str::to_string)),
        kind: Set(event.kind),
        source: Set(serde_enum_to_string(event.source)?),
        target: Set(serde_enum_to_string(event.target)?),
        scope: Set(serde_enum_to_string(event.scope)?),
        confidence: Set(event.confidence),
        ttl_sec: Set(event.ttl_sec.min(i64::MAX as u64) as i64),
        decay: Set(serde_json::to_value(event.decay)?),
        deltas: Set(serde_json::to_value(event.deltas)?),
        evidence: Set(evidence),
        created_at: Set(event.created_at.into()),
    })
}

fn appraisal_from_model(
    model: expression_appraisal_event::Model,
) -> anyhow::Result<PersistedAppraisal> {
    Ok(PersistedAppraisal {
        channel_id: model.channel_id,
        user_id: model.user_id,
        event: AppraisalEvent {
            id: model.id,
            kind: model.kind,
            source: serde_json::from_value(serde_json::Value::String(model.source))?,
            target: serde_json::from_value(serde_json::Value::String(model.target))?,
            scope: serde_json::from_value(serde_json::Value::String(model.scope))?,
            confidence: model.confidence.clamp(0.0, 1.0),
            created_at: model.created_at.with_timezone(&Utc),
            ttl_sec: model.ttl_sec.max(1) as u64,
            decay: serde_json::from_value(model.decay)?,
            deltas: serde_json::from_value(model.deltas)?,
            evidence: serde_json::from_value(model.evidence)?,
        },
    })
}

fn serde_enum_to_string<T: Serialize>(value: T) -> anyhow::Result<String> {
    let value = serde_json::to_value(value)?;
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("expected enum to serialize as string"))
}

fn max_appraisal_ttl_sec(policy: &AffectPolicySection) -> u64 {
    policy
        .turn_appraisal_ttl_sec
        .max(policy.session_appraisal_ttl_sec)
        .max(policy.relationship_appraisal_ttl_sec)
        .max(1)
}

fn max_persisted_retention_sec(policy: &AffectPolicySection) -> u64 {
    if policy.relationship_persistence_enabled {
        max_appraisal_ttl_sec(policy)
    } else {
        policy
            .turn_appraisal_ttl_sec
            .max(policy.session_appraisal_ttl_sec)
            .max(1)
    }
}

fn evidence_for_persistence(
    event: &AppraisalEvent,
    policy: &AffectPolicySection,
) -> anyhow::Result<serde_json::Value> {
    if !policy.persist_appraisal_evidence
        || (is_private_long_term_scope(event.scope) && !policy.persist_relationship_evidence)
    {
        return serde_json::to_value(AppraisalEvidence {
            summary: "redacted".to_string(),
        })
        .map_err(Into::into);
    }

    let max_chars = policy.max_persisted_evidence_chars;
    let summary = if max_chars == 0 {
        "redacted".to_string()
    } else {
        truncate_chars(&event.evidence.summary, max_chars)
    };
    serde_json::to_value(AppraisalEvidence { summary }).map_err(Into::into)
}

fn scope_persistence_allowed(scope: AppraisalScope, policy: &AffectPolicySection) -> bool {
    policy.relationship_persistence_enabled || !is_long_term_scope(scope)
}

fn is_long_term_scope(scope: AppraisalScope) -> bool {
    matches!(
        scope,
        AppraisalScope::User
            | AppraisalScope::Relationship
            | AppraisalScope::Device
            | AppraisalScope::Global
    )
}

fn is_private_long_term_scope(scope: AppraisalScope) -> bool {
    matches!(scope, AppraisalScope::User | AppraisalScope::Relationship)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let mut truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        truncated.push_str("...");
    }
    truncated
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, Utc};

    use super::*;

    #[test]
    fn gratitude_lifts_valence_and_social_engagement() {
        let store = AffectStore::new(AffectPolicySection::default());
        let before = store.snapshot("cli", "user");
        let after = observe_user_text(&store, "cli", "user", "谢谢你，真棒").snapshot;

        assert!(after.affect.valence > before.affect.valence);
        assert!(after.social.engagement > before.social.engagement);
        assert!(
            after
                .dominant_causes
                .iter()
                .any(|cause| cause == "user_gratitude")
        );
    }

    #[test]
    fn relationship_state_follows_user_across_channels() {
        let store = AffectStore::new(AffectPolicySection::default());
        let before = store.snapshot("cli", "user");
        observe_user_text(&store, "embedded", "user", "谢谢你，真棒");
        let after = store.snapshot("cli", "user");

        assert!(after.social.trust > before.social.trust);
        assert!(after.social.familiarity > before.social.familiarity);
    }

    #[test]
    fn device_scope_is_channel_bound() {
        let store = AffectStore::new(AffectPolicySection::default());
        let policy = store.policy();
        let event = AppraisalEvent::new(
            "device_battery_stress",
            AppraisalSource::Runtime,
            AppraisalTarget::Runtime,
            1.0,
            AppraisalScope::Device,
            Utc::now(),
            policy,
            AffectDeltas {
                battery_stress: 0.5,
                ..AffectDeltas::default()
            },
            "Device battery saver enabled",
        );

        store.observe_user_appraisals(
            AffectUserInput {
                channel_id: "embedded",
                user_id: "user",
                user_text: "",
            },
            vec![event],
        );

        assert!(store.snapshot("embedded", "user").resource.battery_stress > 0.0);
        assert_eq!(
            store.snapshot("cli", "user").resource.battery_stress,
            policy.baseline_battery_stress
        );
    }

    #[test]
    fn global_scope_affects_all_channels() {
        let store = AffectStore::new(AffectPolicySection::default());
        let event = AppraisalEvent::new(
            "global_thermal_stress",
            AppraisalSource::Runtime,
            AppraisalTarget::Runtime,
            1.0,
            AppraisalScope::Global,
            Utc::now(),
            store.policy(),
            AffectDeltas {
                thermal_stress: 0.4,
                ..AffectDeltas::default()
            },
            "Server marked global thermal stress",
        );

        store.observe_user_appraisals(
            AffectUserInput {
                channel_id: "embedded",
                user_id: "user",
                user_text: "",
            },
            vec![event],
        );

        assert!(store.snapshot("embedded", "user").resource.thermal_stress > 0.0);
        assert!(store.snapshot("cli", "other").resource.thermal_stress > 0.0);
    }

    #[test]
    fn user_scope_defaults_to_sticky_decay() {
        let policy = AffectPolicySection::default();
        let event = AppraisalEvent::new(
            "user_preference",
            AppraisalSource::UserMessage,
            AppraisalTarget::Assistant,
            1.0,
            AppraisalScope::User,
            Utc::now(),
            &policy,
            AffectDeltas {
                familiarity: 0.1,
                ..AffectDeltas::default()
            },
            "User preference signal",
        );

        assert_eq!(event.decay.kind, AppraisalDecayKind::Sticky);
    }

    #[test]
    fn relationship_profile_tracks_privacy_and_preferences() {
        let store = AffectStore::new(AffectPolicySection::default());
        let event = AppraisalEvent::new(
            "user_preference_voice",
            AppraisalSource::UserMessage,
            AppraisalTarget::Assistant,
            1.0,
            AppraisalScope::User,
            Utc::now(),
            store.policy(),
            AffectDeltas {
                familiarity: 0.1,
                ..AffectDeltas::default()
            },
            "Prefers concise replies",
        );

        store.apply_appraisals("cli", "user", None, Utc::now(), vec![event]);
        let profile = store.relationship_profile("user");

        assert_eq!(profile.user_id, "user");
        assert!(profile.familiarity > store.policy.baseline_familiarity);
        assert!(profile.privacy.user_visible);
        assert!(!profile.privacy.stores_raw_evidence);
        assert_eq!(
            profile.preferences.get("user_preference_voice"),
            Some(&"Prefers concise replies".to_string())
        );
        assert!(profile.summary.contains("preferences=1"));
        assert!(profile.preference_summary.contains("1 durable preference"));
        assert!(profile.privacy.clearable);
    }

    #[tokio::test]
    async fn relationship_profile_can_be_cleared_by_user_privacy_action() {
        let store = AffectStore::new(AffectPolicySection::default());
        observe_user_text(&store, "cli", "user", "谢谢你，真棒");

        let before = store.relationship_profile("user");
        assert!(before.trust > store.policy.baseline_trust);

        let result = store.clear_relationship_data("user").await.unwrap();
        let after = store.relationship_profile("user");

        assert!(result.profile_removed);
        assert!(result.relationship_events_removed > 0);
        assert_eq!(after.trust, store.policy.baseline_trust);
    }

    #[test]
    fn stale_relationship_profiles_are_pruned_by_policy() {
        let policy = AffectPolicySection {
            relationship_profile_prune_after_sec: 1,
            ..AffectPolicySection::default()
        };
        let store = AffectStore::new(policy);
        store.relationship_profiles.write().unwrap().insert(
            "stale".to_string(),
            RelationshipProfile {
                user_id: "stale".to_string(),
                trust: 0.5,
                familiarity: 0.5,
                summary: "old".to_string(),
                preference_summary: String::new(),
                preferences: BTreeMap::new(),
                signal_counts: BTreeMap::new(),
                privacy: RelationshipPrivacyControls {
                    user_visible: true,
                    stores_preferences: true,
                    stores_raw_evidence: false,
                    clearable: true,
                },
                updated_at: (Utc::now() - Duration::seconds(5)).to_rfc3339(),
            },
        );
        store
            .relationships
            .write()
            .unwrap()
            .insert("stale".to_string(), AffectState::default());

        let pruned = store.prune_relationship_profiles();

        assert_eq!(pruned.profiles_removed, 1);
        assert_eq!(pruned.relationship_states_removed, 1);
    }

    #[test]
    fn user_scope_non_relationship_deltas_reach_effective_snapshot() {
        let store = AffectStore::new(AffectPolicySection::default());
        let before = store.snapshot("cli", "user");
        let event = AppraisalEvent::new(
            "user_long_term_engagement",
            AppraisalSource::UserMessage,
            AppraisalTarget::Dialogue,
            1.0,
            AppraisalScope::User,
            Utc::now(),
            store.policy(),
            AffectDeltas {
                valence: 0.08,
                engagement: 0.08,
                interaction_load: 0.04,
                ..AffectDeltas::default()
            },
            "User long-term interaction style signal",
        );

        store.observe_user_appraisals(
            AffectUserInput {
                channel_id: "embedded",
                user_id: "user",
                user_text: "",
            },
            vec![event],
        );
        let after = store.snapshot("cli", "user");

        assert!(after.affect.valence > before.affect.valence);
        assert!(after.social.engagement > before.social.engagement);
        assert!(after.resource.interaction_load > before.resource.interaction_load);
    }

    #[test]
    fn frustration_overrides_affirmation_appraisal() {
        let store = AffectStore::new(AffectPolicySection::default());
        let observation = observe_user_text(&store, "cli", "user", "你真厉害啊，这都能答错");

        assert!(
            observation
                .appraisals
                .iter()
                .any(|event| event.kind == "user_frustration")
        );
        assert!(
            !observation
                .appraisals
                .iter()
                .any(|event| event.kind == "user_affirmation")
        );
        assert!(
            observation.snapshot.affect.valence < AffectPolicySection::default().baseline_valence
        );
    }

    #[test]
    fn expired_events_do_not_affect_snapshot() {
        let policy = AffectPolicySection {
            session_appraisal_ttl_sec: 1,
            ..AffectPolicySection::default()
        };
        let now = Utc::now();
        let mut state = AffectState::default();
        state.append_appraisals(
            vec![AppraisalEvent::new(
                "old_positive_event",
                AppraisalSource::UserMessage,
                AppraisalTarget::Assistant,
                1.0,
                AppraisalScope::Session,
                now - Duration::seconds(5),
                &policy,
                AffectDeltas {
                    valence: 0.6,
                    ..AffectDeltas::default()
                },
                "Old event",
            )],
            now,
            &policy,
        );

        let snapshot = state.snapshot(&policy, now);
        assert_eq!(snapshot.affect.valence, policy.baseline_valence);
        assert_eq!(snapshot.dominant_causes, vec!["baseline".to_string()]);
    }

    #[test]
    fn single_event_delta_is_rate_limited() {
        let policy = AffectPolicySection {
            baseline_valence: 0.0,
            homeostasis_rate: 0.0,
            max_delta_per_turn: 0.1,
            ..AffectPolicySection::default()
        };
        let now = Utc::now();
        let mut state = AffectState::default();
        state.append_appraisals(
            vec![appraisal_event(
                "strong_positive",
                now,
                &policy,
                AffectDeltas {
                    valence: 1.0,
                    ..AffectDeltas::default()
                },
            )],
            now,
            &policy,
        );

        let snapshot = state.snapshot(&policy, now);

        assert!(snapshot.affect.valence <= policy.max_delta_per_turn + f32::EPSILON);
        assert!(snapshot.affect.valence > 0.0);
    }

    #[test]
    fn nonlinear_saturation_softens_near_bounds() {
        let policy = AffectPolicySection {
            baseline_valence: 0.95,
            homeostasis_rate: 0.0,
            max_delta_per_turn: 0.2,
            ..AffectPolicySection::default()
        };
        let now = Utc::now();
        let mut state = AffectState::default();
        state.append_appraisals(
            vec![appraisal_event(
                "near_upper_bound",
                now,
                &policy,
                AffectDeltas {
                    valence: 1.0,
                    ..AffectDeltas::default()
                },
            )],
            now,
            &policy,
        );

        let snapshot = state.snapshot(&policy, now);

        assert!(snapshot.affect.valence > policy.baseline_valence);
        assert!(snapshot.affect.valence < 0.98);
    }

    #[test]
    fn relationship_axes_update_more_slowly_than_affect_axes() {
        let policy = AffectPolicySection {
            baseline_valence: 0.0,
            baseline_trust: 0.5,
            homeostasis_rate: 0.0,
            max_delta_per_turn: 0.2,
            ..AffectPolicySection::default()
        };
        let now = Utc::now();
        let mut state = AffectState::default();
        state.append_appraisals(
            vec![appraisal_event(
                "strong_social_positive",
                now,
                &policy,
                AffectDeltas {
                    valence: 1.0,
                    trust: 1.0,
                    ..AffectDeltas::default()
                },
            )],
            now,
            &policy,
        );

        let snapshot = state.snapshot(&policy, now);
        let affect_delta = snapshot.affect.valence - policy.baseline_valence;
        let trust_delta = snapshot.social.trust - policy.baseline_trust;

        assert!(affect_delta > 0.0);
        assert!(trust_delta > 0.0);
        assert!(trust_delta < affect_delta);
        assert!(trust_delta <= policy.max_delta_per_turn * RELATIONSHIP_DELTA_SCALE);
    }

    #[test]
    fn persisted_appraisal_model_round_trips_to_event() {
        let now = Utc::now();
        let id = Uuid::new_v4();
        let model = expression_appraisal_event::Model {
            id,
            channel_id: "embedded".to_string(),
            user_id: "user".to_string(),
            route: Some("minor_llm".to_string()),
            kind: "user_gratitude".to_string(),
            source: "user_message".to_string(),
            target: "assistant".to_string(),
            scope: "session".to_string(),
            confidence: 0.86,
            ttl_sec: 900,
            decay: serde_json::to_value(AppraisalDecay {
                kind: AppraisalDecayKind::Exponential,
                half_life_sec: 180,
            })
            .unwrap(),
            deltas: serde_json::to_value(AffectDeltas {
                valence: 0.08,
                engagement: 0.06,
                ..AffectDeltas::default()
            })
            .unwrap(),
            evidence: serde_json::to_value(AppraisalEvidence {
                summary: "User expressed gratitude".to_string(),
            })
            .unwrap(),
            created_at: now.into(),
        };

        let persisted = appraisal_from_model(model).unwrap();

        assert_eq!(persisted.channel_id, "embedded");
        assert_eq!(persisted.user_id, "user");
        assert_eq!(persisted.event.id, id);
        assert_eq!(persisted.event.source, AppraisalSource::UserMessage);
        assert_eq!(persisted.event.target, AppraisalTarget::Assistant);
        assert_eq!(persisted.event.scope, AppraisalScope::Session);
        assert_eq!(persisted.event.decay.kind, AppraisalDecayKind::Exponential);
        assert!(persisted.event.deltas.valence > 0.0);
    }

    #[test]
    fn relationship_evidence_is_redacted_for_persistence_by_default() {
        let now = Utc::now();
        let policy = AffectPolicySection::default();
        let event = AppraisalEvent::new(
            "relationship_signal",
            AppraisalSource::UserMessage,
            AppraisalTarget::Dialogue,
            1.0,
            AppraisalScope::Relationship,
            now,
            &policy,
            AffectDeltas {
                trust: 0.1,
                ..AffectDeltas::default()
            },
            "User shared a very specific private detail",
        );

        let model =
            appraisal_to_active_model("cli", "user", None, event, &policy).expect("active model");

        let evidence = model.evidence.unwrap();
        let evidence: AppraisalEvidence = serde_json::from_value(evidence).unwrap();
        assert_eq!(evidence.summary, "redacted");
    }

    #[test]
    fn persisted_evidence_is_truncated_when_enabled() {
        let now = Utc::now();
        let policy = AffectPolicySection {
            max_persisted_evidence_chars: 8,
            ..AffectPolicySection::default()
        };
        let event = AppraisalEvent::new(
            "session_signal",
            AppraisalSource::UserMessage,
            AppraisalTarget::Dialogue,
            1.0,
            AppraisalScope::Session,
            now,
            &policy,
            AffectDeltas {
                valence: 0.1,
                ..AffectDeltas::default()
            },
            "This summary is longer than the persistence limit",
        );

        let model =
            appraisal_to_active_model("cli", "user", None, event, &policy).expect("active model");

        let evidence = model.evidence.unwrap();
        let evidence: AppraisalEvidence = serde_json::from_value(evidence).unwrap();
        assert_eq!(evidence.summary, "This sum...");
    }

    fn observe_user_text(
        store: &AffectStore,
        channel_id: &str,
        user_id: &str,
        user_text: &str,
    ) -> AffectObservation {
        let appraisals = crate::expression::appraisal::rule_based_user_appraisals(
            user_text,
            Utc::now(),
            store.policy(),
        );
        store.observe_user_appraisals(
            AffectUserInput {
                channel_id,
                user_id,
                user_text,
            },
            appraisals,
        )
    }

    fn appraisal_event(
        kind: impl Into<String>,
        now: DateTime<Utc>,
        policy: &AffectPolicySection,
        deltas: AffectDeltas,
    ) -> AppraisalEvent {
        AppraisalEvent::new(
            kind,
            AppraisalSource::UserMessage,
            AppraisalTarget::Dialogue,
            1.0,
            AppraisalScope::Session,
            now,
            policy,
            deltas,
            "Test appraisal",
        )
    }
}
