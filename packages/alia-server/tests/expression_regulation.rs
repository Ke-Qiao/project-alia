use alia_server::config::RegulationPolicySection;
use alia_server::expression::regulation::regulate_expression;
use alia_server::expression::{AffectAxes, AffectSnapshot, ResourceState, SocialState};

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
