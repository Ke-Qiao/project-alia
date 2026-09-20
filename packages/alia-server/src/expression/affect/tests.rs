use super::*;
use chrono::Duration;

fn event(
    kind: &str,
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
        "test_reason",
    )
}

#[test]
fn gratitude_and_frustration_have_distinct_bounded_effects() {
    let policy = AffectPolicySection::default();
    let now = Utc::now();
    let good = crate::expression::appraisal::rule_based_user_appraisals(
        "thank you, awesome",
        now,
        &policy,
    );
    let bad = crate::expression::appraisal::rule_based_user_appraisals(
        "great job, you even got this wrong",
        now,
        &policy,
    );
    assert!(good.iter().any(|event| event.kind == "user_gratitude"));
    assert!(bad.iter().any(|event| event.kind == "user_frustration"));
    assert!(!bad.iter().any(|event| event.kind == "user_affirmation"));
    let (positive, _) = reduce_validated_events(Uuid::nil(), good, &policy, now);
    let (negative, _) = reduce_validated_events(Uuid::nil(), bad, &policy, now);
    assert!(positive.affect.valence > policy.baseline_valence);
    assert!(positive.social.engagement > policy.baseline_engagement);
    assert!(negative.affect.valence < policy.baseline_valence);
}

#[test]
fn expired_events_are_absent_from_snapshot_and_relationship() {
    let policy = AffectPolicySection::default();
    let now = Utc::now();
    let mut old = event(
        "old_preference",
        now - Duration::seconds(5),
        &policy,
        AffectDeltas {
            valence: 0.6,
            trust: 0.5,
            ..Default::default()
        },
    );
    old.scope = AppraisalScope::User;
    old.ttl_sec = 1;
    let (snapshot, profile) = reduce_validated_events(Uuid::nil(), vec![old], &policy, now);
    assert_eq!(snapshot.affect.valence, policy.baseline_valence);
    assert_eq!(snapshot.dominant_causes, ["baseline"]);
    assert_eq!(profile.trust, policy.baseline_trust);
    assert!(profile.preferences.is_empty());
    assert!(profile.signal_counts.is_empty());
}

#[test]
fn single_event_delta_is_rate_limited() {
    let policy = AffectPolicySection {
        baseline_valence: 0.0,
        homeostasis_rate: 0.0,
        max_delta_per_turn: 0.1,
        ..Default::default()
    };
    let now = Utc::now();
    let (snapshot, _) = reduce_validated_events(
        Uuid::nil(),
        vec![event(
            "strong",
            now,
            &policy,
            AffectDeltas {
                valence: 1.0,
                ..Default::default()
            },
        )],
        &policy,
        now,
    );
    assert!(snapshot.affect.valence > 0.0);
    assert!(snapshot.affect.valence <= policy.max_delta_per_turn + f32::EPSILON);
}

#[test]
fn nonlinear_saturation_softens_near_bounds() {
    let policy = AffectPolicySection {
        baseline_valence: 0.95,
        homeostasis_rate: 0.0,
        max_delta_per_turn: 0.2,
        ..Default::default()
    };
    let now = Utc::now();
    let (snapshot, _) = reduce_validated_events(
        Uuid::nil(),
        vec![event(
            "strong",
            now,
            &policy,
            AffectDeltas {
                valence: 1.0,
                ..Default::default()
            },
        )],
        &policy,
        now,
    );
    assert!(snapshot.affect.valence > policy.baseline_valence);
    assert!(snapshot.affect.valence < 0.98);
}

#[test]
fn relationship_axes_change_more_slowly_than_affect() {
    let policy = AffectPolicySection {
        baseline_valence: 0.0,
        baseline_trust: 0.5,
        homeostasis_rate: 0.0,
        max_delta_per_turn: 0.2,
        ..Default::default()
    };
    let now = Utc::now();
    let (snapshot, _) = reduce_validated_events(
        Uuid::nil(),
        vec![event(
            "social",
            now,
            &policy,
            AffectDeltas {
                valence: 1.0,
                trust: 1.0,
                ..Default::default()
            },
        )],
        &policy,
        now,
    );
    let trust = snapshot.social.trust - policy.baseline_trust;
    assert!(trust > 0.0);
    assert!(trust < snapshot.affect.valence - policy.baseline_valence);
    assert!(trust <= policy.max_delta_per_turn * RELATIONSHIP_DELTA_SCALE);
}

#[test]
fn replay_order_comes_from_canonical_time_and_identity() {
    let policy = AffectPolicySection::default();
    let now = Utc::now();
    let mut positive = event(
        "positive",
        now,
        &policy,
        AffectDeltas {
            valence: 0.7,
            ..Default::default()
        },
    );
    positive.id = Uuid::from_u128(1);
    let mut negative = event(
        "negative",
        now,
        &policy,
        AffectDeltas {
            valence: -0.9,
            ..Default::default()
        },
    );
    negative.id = Uuid::from_u128(2);
    let left = reduce_validated_events(
        Uuid::nil(),
        vec![positive.clone(), negative.clone()],
        &policy,
        now,
    );
    let right = reduce_validated_events(Uuid::nil(), vec![negative, positive], &policy, now);
    assert_eq!(
        serde_json::to_value(left).unwrap(),
        serde_json::to_value(right).unwrap()
    );
}

#[test]
fn relationship_profile_uses_live_reason_codes_and_never_private_evidence() {
    let policy = AffectPolicySection::default();
    let now = Utc::now();
    let owner = Uuid::new_v4();
    let mut preference = event(
        "user_preference_voice",
        now,
        &policy,
        AffectDeltas {
            familiarity: 0.1,
            ..Default::default()
        },
    );
    preference.scope = AppraisalScope::User;
    preference.evidence.summary = "a private sentence must not become a profile".into();
    let (_, profile) = reduce_validated_events(owner, vec![preference], &policy, now);
    assert_eq!(profile.user_id, owner.to_string());
    assert_eq!(
        profile.preferences["user_preference_voice"],
        "user_preference_voice"
    );
    assert!(
        !serde_json::to_string(&profile)
            .unwrap()
            .contains("private sentence")
    );
    assert!(!profile.privacy.stores_raw_evidence);
    let (_, rebuilt) = reduce_validated_events(owner, vec![], &policy, now);
    assert!(rebuilt.preferences.is_empty());
    assert_eq!(rebuilt.familiarity, policy.baseline_familiarity);
}

#[test]
fn profile_does_not_promote_turn_scoped_signals_to_relationship() {
    let policy = AffectPolicySection::default();
    let now = Utc::now();
    let turn = event(
        "turn_preference",
        now,
        &policy,
        AffectDeltas {
            trust: 0.4,
            ..Default::default()
        },
    );
    let (snapshot, profile) = reduce_validated_events(Uuid::nil(), vec![turn], &policy, now);
    assert!(snapshot.social.trust > policy.baseline_trust);
    assert_eq!(profile.trust, policy.baseline_trust);
    assert!(profile.preferences.is_empty());
}

#[test]
fn disabled_affect_has_no_snapshot_effect() {
    let policy = AffectPolicySection {
        enabled: false,
        ..Default::default()
    };
    let now = Utc::now();
    let (snapshot, _) = reduce_validated_events(
        Uuid::nil(),
        vec![event(
            "positive",
            now,
            &policy,
            AffectDeltas {
                valence: 0.9,
                ..Default::default()
            },
        )],
        &policy,
        now,
    );
    assert_eq!(snapshot.affect.valence, policy.baseline_valence);
    assert_eq!(snapshot.dominant_causes, ["baseline"]);
}

#[test]
fn user_scope_uses_policy_sticky_decay() {
    let policy = AffectPolicySection::default();
    let user = AppraisalEvent::new(
        "preference",
        AppraisalSource::UserMessage,
        AppraisalTarget::Assistant,
        1.0,
        AppraisalScope::User,
        Utc::now(),
        &policy,
        AffectDeltas::default(),
        "reason",
    );
    assert_eq!(user.decay.kind, AppraisalDecayKind::Sticky);
    assert_eq!(user.ttl_sec, policy.relationship_appraisal_ttl_sec);
}

#[test]
fn event_is_inactive_at_exact_expiry_and_before_its_source_time() {
    let policy = AffectPolicySection::default();
    let now = Utc::now();
    let mut value = event("bounded", now, &policy, AffectDeltas::default());
    value.ttl_sec = 1;
    assert!(value.is_active(now + Duration::milliseconds(999)));
    assert!(!value.is_active(now + Duration::seconds(1)));
    assert!(!value.is_active(now - Duration::milliseconds(1)));
}
