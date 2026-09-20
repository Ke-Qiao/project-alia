use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::{AffectPolicySection, AppraisalLexiconSection, ServerConfig};

pub const APPRAISAL_INTERPRETER_VERSION: &str = "alia.appraisal.v1";
pub const AFFECT_REDUCER_VERSION: &str = "alia.affect.v1";

/// Persisted inputs to interpretation and replay. Runtime configuration is a
/// seed only; no process may silently substitute its private policy snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppraisalPolicy {
    pub affect: AffectPolicySection,
    pub lexicons: BTreeMap<String, AppraisalLexiconSection>,
    pub interpreter_version: String,
    pub reducer_version: String,
    pub interpretation_timeout_ms: u64,
    /// Server-approved extension identity -> implementation/configuration pin.
    /// Unknown engines cannot obtain authority by declaring their own version.
    pub approved_interpreters: BTreeMap<String, String>,
}

impl Default for AppraisalPolicy {
    fn default() -> Self {
        Self {
            affect: AffectPolicySection::default(),
            lexicons: BTreeMap::new(),
            interpreter_version: APPRAISAL_INTERPRETER_VERSION.into(),
            reducer_version: AFFECT_REDUCER_VERSION.into(),
            interpretation_timeout_ms: 10_000,
            approved_interpreters: BTreeMap::new(),
        }
    }
}

impl AppraisalPolicy {
    pub fn from_config(config: &ServerConfig) -> Self {
        Self {
            affect: config.policies.affect.clone(),
            lexicons: config.resources.appraisal_lexicons.clone(),
            ..Self::default()
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        let p = &self.affect;
        anyhow::ensure!(
            self.interpreter_version.len() <= 128
                && !self.interpreter_version.is_empty()
                && self.reducer_version.len() <= 128
                && !self.reducer_version.is_empty()
                && (1..=10_000).contains(&self.interpretation_timeout_ms)
                && (1..=64).contains(&p.max_active_appraisal_events)
                && p.relationship_preferences_max_items <= 64
                && p.relationship_summary_max_signals <= 64
                && p.max_persisted_evidence_chars <= 1024
                && self.approved_interpreters.len() <= 16,
            "invalid appraisal policy bounds"
        );
        for (name, pin) in &self.approved_interpreters {
            anyhow::ensure!(
                !name.is_empty()
                    && name.len() <= 128
                    && pin.len() == 64
                    && pin.bytes().all(|v| v.is_ascii_hexdigit()),
                "invalid approved interpreter pin"
            );
        }
        let values = [
            p.baseline_valence,
            p.baseline_arousal,
            p.baseline_calmness,
            p.baseline_engagement,
            p.baseline_fatigue,
            p.baseline_battery_stress,
            p.baseline_thermal_stress,
            p.baseline_sleepiness,
            p.baseline_interaction_load,
            p.baseline_trust,
            p.baseline_familiarity,
            p.homeostasis_rate,
            p.max_delta_per_turn,
            p.fatigue_per_turn,
            p.long_response_fatigue_delta,
            p.min_appraisal_confidence,
        ];
        anyhow::ensure!(
            values
                .iter()
                .all(|v| v.is_finite() && (-1.0..=1.0).contains(v)),
            "non-finite or excessive affect policy value"
        );
        anyhow::ensure!(
            [
                p.turn_appraisal_ttl_sec,
                p.session_appraisal_ttl_sec,
                p.relationship_appraisal_ttl_sec,
                p.appraisal_half_life_sec,
                p.relationship_appraisal_half_life_sec
            ]
            .iter()
            .all(|value| (1..=31_536_000).contains(value)),
            "invalid appraisal lifetime"
        );
        anyhow::ensure!(
            serde_json::to_vec(self)?.len() <= 24 * 1024,
            "appraisal policy exceeds its byte budget"
        );
        Ok(())
    }

    pub fn interpretation_digest(&self) -> anyhow::Result<String> {
        super::repository::digest(&serde_json::to_value(self)?)
    }
}
