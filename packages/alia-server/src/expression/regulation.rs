use crate::config::RegulationPolicySection;

use super::types::{AffectSnapshot, ExpressiveStance};

pub fn regulate_expression(
    affect: &AffectSnapshot,
    social_function: &str,
    base_style: &str,
    policy: &RegulationPolicySection,
) -> ExpressiveStance {
    if !policy.enabled {
        return unregulated_stance(affect, policy);
    }

    let mut stance = base_stance(affect, policy);
    if social_function == "warn" {
        stance.strategy = "clear_serious".to_string();
        stance.style_hint = Some("serious".to_string());
        stance.max_intensity = stance.max_intensity.min(policy.warning_max_intensity);
        stance.motion_scale = stance.motion_scale.min(0.35);
        stance.reasons.push("warning_policy".to_string());
        return normalize_stance(stance);
    }

    let resource_pressure = affect
        .resource
        .fatigue
        .max(affect.resource.battery_stress)
        .max(affect.resource.thermal_stress)
        .max(affect.resource.sleepiness)
        .max(affect.resource.interaction_load);
    if resource_pressure >= policy.fatigue_threshold {
        stance.strategy = "conserve_energy".to_string();
        stance.style_hint = Some("calm".to_string());
        stance.expressed_arousal = stance.expressed_arousal.min(0.32);
        stance.max_intensity = stance.max_intensity.min(policy.fatigue_max_intensity);
        stance.motion_scale = stance.motion_scale.min(policy.fatigue_motion_scale);
        stance.rate_offset -= 0.04;
        stance.energy_offset -= 0.08;
        stance.pause_bias_offset += 0.06;
        stance.reasons.push("resource_regulation".to_string());
    }

    if affect.social.trust <= policy.low_trust_threshold {
        stance.strategy = "careful_warmth".to_string();
        stance.style_hint = Some("gentle".to_string());
        stance.expressed_valence = stance.expressed_valence.max(0.02);
        stance.expressed_arousal = stance.expressed_arousal.min(0.38);
        stance.warmth = stance.warmth.max(policy.low_trust_warmth_floor);
        stance.max_intensity = stance.max_intensity.min(policy.low_trust_max_intensity);
        stance.motion_scale = stance.motion_scale.min(policy.low_trust_motion_scale);
        stance.pause_bias_offset += 0.04;
        stance.reasons.push("low_trust_regulation".to_string());
    }

    if affect.affect.valence < -policy.max_negative_expression {
        stance.strategy = "deescalate".to_string();
        stance.style_hint = if affect.affect.arousal >= policy.deescalate_arousal_threshold {
            Some("calm".to_string())
        } else {
            Some("gentle".to_string())
        };
        stance.expressed_valence = stance
            .expressed_valence
            .max(-policy.max_negative_expression);
        stance.expressed_arousal = stance.expressed_arousal.min(policy.deescalated_arousal);
        stance.warmth = stance.warmth.max(policy.deescalation_warmth_floor);
        stance.max_intensity = stance.max_intensity.min(policy.deescalated_max_intensity);
        stance.motion_scale = stance.motion_scale.min(policy.deescalated_motion_scale);
        stance.energy_offset -= 0.05;
        stance.pause_bias_offset += 0.05;
        stance
            .reasons
            .push("negative_affect_regulation".to_string());
    }

    if matches!(social_function, "apologize") {
        stance.style_hint = Some("gentle".to_string());
        stance.max_intensity = stance.max_intensity.min(policy.apology_max_intensity);
        stance.motion_scale = stance.motion_scale.min(policy.apology_motion_scale);
        stance.pause_bias_offset += 0.04;
        stance.reasons.push("apology_regulation".to_string());
    } else if matches!(social_function, "celebrate")
        && affect.social.trust >= policy.open_trust_threshold
        && affect.social.familiarity >= policy.open_familiarity_threshold
        && affect.affect.valence > 0.0
    {
        stance.strategy = "open_positive".to_string();
        stance.style_hint = Some("cheerful".to_string());
        stance.intensity_scale *= policy.open_positive_intensity_scale;
        stance.motion_scale *= policy.open_positive_motion_scale;
        stance.energy_offset += 0.03;
        stance
            .reasons
            .push("positive_relationship_opening".to_string());
    }

    if base_style == "calm" {
        stance.motion_scale = stance.motion_scale.min(0.75);
    }

    normalize_stance(stance)
}

fn base_stance(affect: &AffectSnapshot, policy: &RegulationPolicySection) -> ExpressiveStance {
    ExpressiveStance {
        strategy: "regulated".to_string(),
        style_hint: None,
        expressed_valence: affect
            .affect
            .valence
            .clamp(-policy.max_negative_expression, 1.0),
        expressed_arousal: affect
            .affect
            .arousal
            .clamp(0.0, policy.max_expressed_arousal),
        warmth: (0.45 + affect.social.trust * 0.35 + affect.social.familiarity * 0.15)
            .clamp(0.0, 1.0),
        intensity_scale: 1.0,
        max_intensity: policy.default_max_intensity.clamp(0.0, 1.0),
        motion_scale: 1.0,
        rate_offset: 0.0,
        energy_offset: 0.0,
        pause_bias_offset: 0.0,
        reasons: Vec::new(),
    }
}

fn unregulated_stance(
    affect: &AffectSnapshot,
    policy: &RegulationPolicySection,
) -> ExpressiveStance {
    ExpressiveStance {
        strategy: "unregulated".to_string(),
        style_hint: None,
        expressed_valence: affect.affect.valence.clamp(-1.0, 1.0),
        expressed_arousal: affect.affect.arousal.clamp(0.0, 1.0),
        warmth: affect.social.trust.clamp(0.0, 1.0),
        max_intensity: policy.default_max_intensity.clamp(0.0, 1.0),
        reasons: vec!["regulation_disabled".to_string()],
        ..ExpressiveStance::default()
    }
}

fn normalize_stance(mut stance: ExpressiveStance) -> ExpressiveStance {
    stance.expressed_valence = stance.expressed_valence.clamp(-1.0, 1.0);
    stance.expressed_arousal = stance.expressed_arousal.clamp(0.0, 1.0);
    stance.warmth = stance.warmth.clamp(0.0, 1.0);
    stance.intensity_scale = stance.intensity_scale.clamp(0.0, 1.4);
    stance.max_intensity = stance.max_intensity.clamp(0.0, 1.0);
    stance.motion_scale = stance.motion_scale.clamp(0.0, 1.2);
    stance.rate_offset = stance.rate_offset.clamp(-0.25, 0.25);
    stance.energy_offset = stance.energy_offset.clamp(-0.3, 0.3);
    stance.pause_bias_offset = stance.pause_bias_offset.clamp(-0.3, 0.3);
    if stance.reasons.is_empty() {
        stance.reasons.push("baseline_regulation".to_string());
    }
    stance
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::{AffectAxes, ResourceState, SocialState};

    #[test]
    fn negative_high_arousal_state_is_deescalated() {
        let snapshot = AffectSnapshot {
            affect: AffectAxes {
                valence: -0.7,
                arousal: 0.9,
                calmness: 0.1,
            },
            social: SocialState {
                trust: 0.5,
                familiarity: 0.2,
                engagement: 0.5,
            },
            resource: ResourceState {
                fatigue: 0.1,
                ..ResourceState::default()
            },
            dominant_causes: vec!["user_frustration".to_string()],
        };

        let stance = regulate_expression(
            &snapshot,
            "acknowledge",
            "gentle",
            &RegulationPolicySection::default(),
        );

        assert_eq!(stance.strategy, "deescalate");
        assert!(
            stance.expressed_valence >= -RegulationPolicySection::default().max_negative_expression
        );
        assert!(stance.expressed_arousal <= RegulationPolicySection::default().deescalated_arousal);
        assert_eq!(stance.style_hint.as_deref(), Some("calm"));
    }
}
