use super::*;

fn at() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-26T04:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn source() -> CanonicalObservation {
    CanonicalObservation {
        owner_id: Uuid::from_u128(1),
        context_id: Uuid::from_u128(2),
        request_id: Uuid::from_u128(3),
        source_event_id: Uuid::from_u128(4),
        conversation_id: Uuid::from_u128(5),
        channel_id: "paired-pi".into(),
        source_received_at: at() - Duration::seconds(1),
        effective_at: at(),
        privacy_epoch: 3,
        relationship_epoch: 2,
        context_policy_version: 7,
        appraisals: vec![AppraisalReference {
            appraisal_id: Uuid::from_u128(6),
            revision: 1,
            result_digest: "a".repeat(64),
        }],
        valid_until: at() + Duration::minutes(5),
    }
}

fn decision_input(mode: CompanionMode) -> DecisionInput {
    let policy = PolicySnapshot {
        revision: 1,
        policy: CompanionPolicy {
            mode,
            ..CompanionPolicy::default()
        },
    };
    DecisionInput {
        candidate: generate_candidate(&source(), &policy, at()).unwrap(),
        current_policy: policy,
        as_of: at(),
        opportunity: OpportunitySnapshot::simulation(at()),
        budget: BudgetSnapshot::empty(at()),
    }
}

fn live_input() -> DecisionInput {
    let mut input = decision_input(CompanionMode::Enabled);
    input.opportunity.source = OpportunitySource::VerifiedLive {
        server_instance_id: Uuid::from_u128(20),
        device_boot_id: Uuid::from_u128(21),
        session_id: Uuid::from_u128(22),
        generation_epoch: 1,
    };
    input.opportunity.capture_revision = Some(10);
    input.opportunity.last_capture_at = Some(at());
    input
}

#[test]
fn simulated_opportunity_never_allows_output_and_defaults_remain_shadow() {
    assert_eq!(CompanionPolicy::default().mode, CompanionMode::Shadow);
    for mode in [
        CompanionMode::Disabled,
        CompanionMode::Shadow,
        CompanionMode::Enabled,
    ] {
        let decision = decide(&decision_input(mode)).unwrap();
        assert_ne!(decision.outcome, DecisionOutcome::Allowed);
        assert!(
            decision
                .reasons
                .contains(&DecisionReason::SimulatedOpportunity)
        );
        assert_eq!(
            decision.outcome,
            if mode == CompanionMode::Disabled {
                DecisionOutcome::Suppressed
            } else {
                DecisionOutcome::ShadowWouldAllow
            }
        );
    }
}

#[test]
fn candidate_identity_survives_policy_and_appraisal_rebuild_without_refreshing_expiry() {
    let original = decision_input(CompanionMode::Shadow).candidate;
    let mut changed_policy = original.policy.clone();
    changed_policy.revision += 1;
    changed_policy.policy.mode = CompanionMode::Enabled;
    let mut rebuilt_source = source();
    rebuilt_source.context_id = Uuid::from_u128(40);
    rebuilt_source.context_policy_version += 1;
    rebuilt_source.appraisals[0].appraisal_id = Uuid::from_u128(41);
    rebuilt_source.appraisals[0].revision += 1;
    let rebuilt = generate_candidate(&rebuilt_source, &changed_policy, at()).unwrap();
    assert_eq!(
        (original.id, &original.dedup_key),
        (rebuilt.id, &rebuilt.dedup_key)
    );
    assert_eq!(original.expires_at, rebuilt.expires_at);
    assert!(
        generate_candidate(
            &rebuilt_source,
            &changed_policy,
            at() + Duration::seconds(1)
        )
        .is_err()
    );
    let mut forged = original;
    forged.expires_at += Duration::seconds(1);
    assert!(forged.validate().is_err());
}

#[test]
fn source_expiry_shortens_candidate_and_canonical_reference_order_is_stable() {
    let policy = decision_input(CompanionMode::Shadow).current_policy;
    let mut observation = source();
    observation.appraisals.push(AppraisalReference {
        appraisal_id: Uuid::from_u128(7),
        revision: 2,
        result_digest: "b".repeat(64),
    });
    observation.valid_until = at() + Duration::seconds(5);
    let first = generate_candidate(&observation, &policy, at()).unwrap();
    observation.appraisals.reverse();
    let second = generate_candidate(&observation, &policy, at()).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.expires_at, observation.valid_until);
    observation
        .appraisals
        .push(observation.appraisals[0].clone());
    assert!(generate_candidate(&observation, &policy, at()).is_err());
}

#[test]
fn old_shadow_cannot_be_promoted_and_policy_content_changes_also_revoke() {
    let mut input = live_input();
    input.candidate = generate_candidate(
        &source(),
        &decision_input(CompanionMode::Shadow).current_policy,
        at(),
    )
    .unwrap();
    let decision = decide(&input).unwrap();
    assert_eq!(decision.outcome, DecisionOutcome::Suppressed);
    assert!(decision.reasons.contains(&DecisionReason::PolicyChanged));
    assert!(decision.reasons.contains(&DecisionReason::ShadowOrigin));

    let mut input = live_input();
    input.current_policy.policy.daily_budget = 4;
    assert!(
        decide(&input)
            .unwrap()
            .reasons
            .contains(&DecisionReason::PolicyChanged)
    );
    let mut input = live_input();
    input.current_policy.revision += 1;
    assert!(
        decide(&input)
            .unwrap()
            .reasons
            .contains(&DecisionReason::PolicyChanged)
    );
}

#[test]
fn hard_gates_override_enabled_and_idle_state() {
    let cases = [
        (ActivityState::Unknown, DecisionReason::ActivityUnknown),
        (ActivityState::UserSpeaking, DecisionReason::UserSpeaking),
        (
            ActivityState::InteractionBusy,
            DecisionReason::InteractionBusy,
        ),
        (ActivityState::OutputBusy, DecisionReason::OutputBusy),
        (ActivityState::OutputHeld, DecisionReason::OutputHeld),
    ];
    for (activity, reason) in cases {
        let mut input = live_input();
        input.opportunity.activity = activity;
        let decision = decide(&input).unwrap();
        assert_eq!(decision.outcome, DecisionOutcome::Suppressed);
        assert!(decision.reasons.contains(&reason));
    }
    let mut input = live_input();
    input.current_policy.policy.quiet = true;
    input.current_policy.policy.paused = true;
    input.candidate.policy = input.current_policy.clone();
    let decision = decide(&input).unwrap();
    assert_eq!(
        decision.reasons,
        vec![DecisionReason::Quiet, DecisionReason::Paused]
    );

    let mut input = live_input();
    input.opportunity = OpportunitySnapshot::unavailable(at());
    let decision = decide(&input).unwrap();
    assert_eq!(decision.outcome, DecisionOutcome::Suppressed);
    assert!(
        decision
            .reasons
            .contains(&DecisionReason::OpportunityUnavailable)
    );
    assert!(decision.reasons.contains(&DecisionReason::OwnerUnverified));
    assert!(
        decision
            .reasons
            .contains(&DecisionReason::CapabilityUnavailable)
    );
}

#[test]
fn live_capture_must_be_fresh_and_limits_the_decision_deadline() {
    let mut input = live_input();
    // Speech revision zero is valid after continuous silence capture.
    input.opportunity.capture_revision = Some(0);
    let fresh = decide(&input).unwrap();
    assert_eq!(fresh.outcome, DecisionOutcome::Allowed);
    assert_eq!(
        fresh.valid_until,
        at() + Duration::seconds(MAX_CAPTURE_AGE_SECONDS)
    );

    input.opportunity.capture_revision = None;
    input.opportunity.last_capture_at = None;
    assert_eq!(
        decide(&input).unwrap().reasons,
        vec![DecisionReason::CaptureUnavailable]
    );
    input.opportunity.capture_revision = Some(10);
    input.opportunity.last_capture_at = Some(at() - Duration::seconds(MAX_CAPTURE_AGE_SECONDS));
    assert_eq!(
        decide(&input).unwrap().reasons,
        vec![DecisionReason::CaptureStale]
    );
}

#[test]
fn expiry_is_half_open_and_invalid_frozen_times_are_rejected() {
    let mut input = decision_input(CompanionMode::Shadow);
    input.as_of = input.candidate.expires_at;
    input.opportunity = OpportunitySnapshot::simulation(input.as_of);
    input.budget = BudgetSnapshot::empty(input.as_of);
    assert!(
        decide(&input)
            .unwrap()
            .reasons
            .contains(&DecisionReason::CandidateExpired)
    );

    let mut input = live_input();
    input.opportunity.valid_until = input.as_of;
    assert!(
        decide(&input)
            .unwrap()
            .reasons
            .contains(&DecisionReason::OpportunityExpired)
    );
    input.opportunity.observed_at += Duration::seconds(1);
    assert!(decide(&input).is_err());
    let mut input = live_input();
    input.budget.as_of -= Duration::seconds(1);
    assert!(decide(&input).is_err());
}

#[test]
fn daily_budget_cooldown_dedup_and_response_gates_are_independent() {
    let mut input = live_input();
    input.budget.admitted_today = 3;
    input.budget.last_admitted_at = Some(at() - Duration::seconds(899));
    input.budget.deduplicated = true;
    input.budget.awaiting_response = true;
    assert_eq!(
        decide(&input).unwrap().reasons,
        vec![
            DecisionReason::DailyBudgetExhausted,
            DecisionReason::Cooldown,
            DecisionReason::Duplicate,
            DecisionReason::AwaitingResponse,
        ]
    );
    input.budget.admitted_today = 2;
    input.budget.last_admitted_at = Some(at() - Duration::seconds(900));
    input.budget.deduplicated = false;
    input.budget.awaiting_response = false;
    assert_eq!(decide(&input).unwrap().outcome, DecisionOutcome::Allowed);
}

#[test]
fn utc_midnight_limits_frozen_budget_validity() {
    let as_of = DateTime::parse_from_rfc3339("2026-09-26T23:59:59Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut input = live_input();
    let mut observation = source();
    observation.effective_at = as_of;
    observation.valid_until = as_of + Duration::minutes(5);
    input.candidate = generate_candidate(&observation, &input.current_policy, as_of).unwrap();
    input.as_of = as_of;
    input.opportunity.observed_at = as_of;
    input.opportunity.valid_until = as_of + Duration::seconds(10);
    input.opportunity.last_capture_at = Some(as_of);
    input.budget = BudgetSnapshot::empty(as_of);
    assert_eq!(
        decide(&input).unwrap().valid_until,
        as_of + Duration::seconds(1)
    );
    input.budget.utc_date = as_of.date_naive().pred_opt().unwrap();
    assert!(decide(&input).is_err());
}

#[test]
fn replay_is_stable_across_json_key_order_and_keeps_nanosecond_observations() {
    let mut input = live_input();
    input.as_of += Duration::nanoseconds(123);
    input.opportunity.observed_at = input.as_of;
    input.opportunity.last_capture_at = Some(input.as_of);
    input.budget = BudgetSnapshot::empty(input.as_of);
    let expected = decide(&input).unwrap();
    let value = serde_json::to_value(&input).unwrap();
    let encoded = serde_json::to_string_pretty(&value).unwrap();
    let restored: DecisionInput = serde_json::from_str(&encoded).unwrap();
    assert_eq!(restored.as_of, input.as_of);
    assert_eq!(decide(&restored).unwrap(), expected);
    assert!(replay(&restored, &expected).unwrap());
    let mut changed = restored;
    changed.budget.revision += 1;
    assert!(!replay(&changed, &expected).unwrap());
    let mut tampered = expected;
    tampered.outcome = DecisionOutcome::Suppressed;
    assert!(replay(&input, &tampered).is_err());
}

#[test]
fn unknown_fields_and_malformed_or_excessive_inputs_are_rejected() {
    let original = live_input();
    let mut value = serde_json::to_value(&original).unwrap();
    value["authority"] = serde_json::json!(true);
    assert!(serde_json::from_value::<DecisionInput>(value).is_err());
    let mut value = serde_json::to_value(&original).unwrap();
    value["candidate"]["observation"]["raw_text"] = serde_json::json!("private");
    assert!(serde_json::from_value::<DecisionInput>(value).is_err());
    let mut value = serde_json::to_value(&original).unwrap();
    value["opportunity"]["source"]["authority"] = serde_json::json!(true);
    assert!(serde_json::from_value::<DecisionInput>(value).is_err());

    let mut invalid = original.clone();
    invalid.current_policy.policy.candidate_ttl_seconds = 301;
    assert!(decide(&invalid).is_err());
    invalid = original.clone();
    invalid.current_policy.policy.cooldown_seconds = 29;
    assert!(decide(&invalid).is_err());
    invalid = original.clone();
    invalid.current_policy.policy.daily_budget = 25;
    assert!(decide(&invalid).is_err());
    invalid = original.clone();
    invalid.candidate.observation.appraisals[0].result_digest = "A".repeat(64);
    assert!(decide(&invalid).is_err());
    invalid = original.clone();
    invalid.candidate.observation.appraisals = (1..=9)
        .map(|index| AppraisalReference {
            appraisal_id: Uuid::from_u128(index),
            revision: 1,
            result_digest: "a".repeat(64),
        })
        .collect();
    assert!(decide(&invalid).is_err());
    invalid = original.clone();
    invalid.candidate.observation.channel_id = "x".repeat(257);
    assert!(decide(&invalid).is_err());
    invalid = original.clone();
    invalid.opportunity.capture_revision = None;
    assert!(decide(&invalid).is_err());
    invalid = original;
    invalid.opportunity.last_capture_at = Some(at() + Duration::seconds(1));
    assert!(decide(&invalid).is_err());
}
