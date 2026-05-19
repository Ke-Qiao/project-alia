use std::collections::BTreeMap;

use alia_server::config::{AffectPolicySection, AppraisalLexiconSection};
use alia_server::expression::classifier_user_appraisals;
use chrono::Utc;

fn test_lexicons() -> BTreeMap<String, AppraisalLexiconSection> {
    let mut lexicons = BTreeMap::new();
    lexicons.insert(
        "en".to_string(),
        AppraisalLexiconSection {
            metadata: Default::default(),
            positive_phrases: vec!["great job".to_string(), "awesome".to_string()],
            correction_or_failure: vec!["wrong".to_string(), "even this".to_string()],
            sarcasm_markers: vec!["great job".to_string()],
            support_need: vec!["anxious".to_string()],
            safety_salience: vec!["danger".to_string()],
            minimal_expression: vec!["brief".to_string()],
        },
    );
    lexicons
}

#[test]
fn classifier_detects_sarcastic_dissatisfaction() {
    let events = classifier_user_appraisals(
        "great job, you even got this wrong.",
        Utc::now(),
        &AffectPolicySection::default(),
        &test_lexicons(),
    );

    assert!(
        events
            .iter()
            .any(|event| event.kind == "user_sarcasm_or_dissatisfaction")
    );
    assert!(
        events
            .iter()
            .any(|event| event.kind == "relationship_repair_needed")
    );
}

#[test]
fn classifier_detects_support_need_and_minimal_preference() {
    let events = classifier_user_appraisals(
        "I feel anxious, please keep the answer brief.",
        Utc::now(),
        &AffectPolicySection::default(),
        &test_lexicons(),
    );

    assert!(events.iter().any(|event| event.kind == "user_support_need"));
    assert!(
        events
            .iter()
            .any(|event| event.kind == "user_requested_minimal_expression")
    );
}
