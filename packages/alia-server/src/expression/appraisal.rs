use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

use crate::config::{AffectPolicySection, AppraisalLexiconSection};

use super::affect::{
    AffectDeltas, AppraisalEvent, AppraisalScope, AppraisalSource, AppraisalTarget,
    filter_appraisals,
};

pub struct AppraisalContext<'a> {
    pub channel_id: &'a str,
    pub user_id: &'a str,
    pub user_text: &'a str,
    pub now: DateTime<Utc>,
    pub policy: &'a AffectPolicySection,
}

/// Extension point for interpreting raw user input into durable appraisal events.
///
/// Implementations may be rules, local classifiers, or LLM-backed interpreters.
/// They should not mutate affect state directly; return `AppraisalEvent`s and let
/// the affect store reduce them.
#[async_trait]
pub trait AppraisalEnginePlugin: Send + Sync {
    fn name(&self) -> &str;

    async fn appraise_user_input(
        &self,
        ctx: &AppraisalContext<'_>,
    ) -> anyhow::Result<Vec<AppraisalEvent>>;
}

#[derive(Debug, Default)]
pub struct RuleBasedAppraisalEngine;

#[async_trait]
impl AppraisalEnginePlugin for RuleBasedAppraisalEngine {
    fn name(&self) -> &str {
        "rule_based_user_appraisal"
    }

    async fn appraise_user_input(
        &self,
        ctx: &AppraisalContext<'_>,
    ) -> anyhow::Result<Vec<AppraisalEvent>> {
        Ok(rule_based_user_appraisals(
            ctx.user_text,
            ctx.now,
            ctx.policy,
        ))
    }
}

/// Lightweight classifier-style appraisal engine for intent/emotional signals
/// that are awkward to express as simple positive/negative keyword rules.
///
/// This is deliberately deterministic and local. Deployments can register an
/// LLM-backed `AppraisalEnginePlugin` later, but this engine gives the runtime a
/// real classifier boundary today: it interprets text into appraisal events
/// without mutating affect state directly.
#[derive(Debug, Default)]
pub struct TextClassifierAppraisalEngine {
    lexicons: BTreeMap<String, AppraisalLexiconSection>,
}

impl TextClassifierAppraisalEngine {
    pub fn new(lexicons: BTreeMap<String, AppraisalLexiconSection>) -> Self {
        Self { lexicons }
    }
}

#[async_trait]
impl AppraisalEnginePlugin for TextClassifierAppraisalEngine {
    fn name(&self) -> &str {
        "text_classifier_appraisal"
    }

    async fn appraise_user_input(
        &self,
        ctx: &AppraisalContext<'_>,
    ) -> anyhow::Result<Vec<AppraisalEvent>> {
        Ok(classifier_user_appraisals(
            ctx.user_text,
            ctx.now,
            ctx.policy,
            &self.lexicons,
        ))
    }
}

pub fn rule_based_user_appraisals(
    text: &str,
    now: DateTime<Utc>,
    policy: &AffectPolicySection,
) -> Vec<AppraisalEvent> {
    let lower = text.to_lowercase();
    let frustrated = contains_any(
        &lower,
        &[
            "烦", "错了", "答错", "不对", "生气", "terrible", "wrong", "annoying",
        ],
    );
    let mut events = Vec::new();

    if contains_any(&lower, &["谢谢", "感谢", "thank", "thanks", "辛苦了"]) {
        events.push(appraisal(
            "user_gratitude",
            AppraisalSource::UserMessage,
            AppraisalTarget::Assistant,
            0.86,
            AppraisalScope::Session,
            now,
            policy,
            AffectDeltas {
                valence: 0.08,
                calmness: 0.02,
                engagement: 0.06,
                ..AffectDeltas::default()
            },
            "User expressed gratitude",
        ));
        events.push(appraisal(
            "relationship_gratitude",
            AppraisalSource::UserMessage,
            AppraisalTarget::Assistant,
            0.78,
            AppraisalScope::User,
            now,
            policy,
            AffectDeltas {
                trust: 0.012,
                familiarity: 0.018,
                ..AffectDeltas::default()
            },
            "User gratitude slightly strengthens relationship context",
        ));
    }
    if !frustrated && contains_any(&lower, &["厉害", "真棒", "喜欢你", "great job", "awesome"])
    {
        events.push(appraisal(
            "user_affirmation",
            AppraisalSource::UserMessage,
            AppraisalTarget::Assistant,
            0.82,
            AppraisalScope::Session,
            now,
            policy,
            AffectDeltas {
                valence: 0.10,
                calmness: 0.015,
                engagement: 0.08,
                ..AffectDeltas::default()
            },
            "User gave positive affirmation",
        ));
        events.push(appraisal(
            "relationship_affirmation",
            AppraisalSource::UserMessage,
            AppraisalTarget::Assistant,
            0.76,
            AppraisalScope::User,
            now,
            policy,
            AffectDeltas {
                trust: 0.014,
                familiarity: 0.020,
                ..AffectDeltas::default()
            },
            "User affirmation slightly strengthens relationship context",
        ));
    }
    if frustrated {
        events.push(appraisal(
            "user_frustration",
            AppraisalSource::UserMessage,
            AppraisalTarget::Dialogue,
            0.84,
            AppraisalScope::Session,
            now,
            policy,
            AffectDeltas {
                valence: -0.12,
                arousal: 0.08,
                calmness: -0.08,
                engagement: 0.03,
                ..AffectDeltas::default()
            },
            "User expressed frustration or disagreement",
        ));
        events.push(appraisal(
            "relationship_friction",
            AppraisalSource::UserMessage,
            AppraisalTarget::Dialogue,
            0.74,
            AppraisalScope::User,
            now,
            policy,
            AffectDeltas {
                trust: -0.018,
                familiarity: 0.006,
                ..AffectDeltas::default()
            },
            "User frustration slightly lowers trust while preserving familiarity",
        ));
    }
    if text.ends_with('?') || text.ends_with('？') {
        events.push(appraisal(
            "user_question",
            AppraisalSource::UserMessage,
            AppraisalTarget::Dialogue,
            0.72,
            AppraisalScope::Turn,
            now,
            policy,
            AffectDeltas {
                engagement: 0.04,
                ..AffectDeltas::default()
            },
            "User asked a question",
        ));
    }
    if text.chars().count() > 80 {
        events.push(appraisal(
            "high_user_engagement",
            AppraisalSource::UserMessage,
            AppraisalTarget::Dialogue,
            0.65,
            AppraisalScope::Session,
            now,
            policy,
            AffectDeltas {
                arousal: 0.02,
                engagement: 0.05,
                ..AffectDeltas::default()
            },
            "User sent a long engaged message",
        ));
        events.push(appraisal(
            "relationship_familiarity_gain",
            AppraisalSource::UserMessage,
            AppraisalTarget::Dialogue,
            0.62,
            AppraisalScope::User,
            now,
            policy,
            AffectDeltas {
                familiarity: 0.012,
                ..AffectDeltas::default()
            },
            "Long engaged message adds a small familiarity signal",
        ));
    }

    filter_appraisals(events, policy)
}

pub fn classifier_user_appraisals(
    text: &str,
    now: DateTime<Utc>,
    policy: &AffectPolicySection,
    lexicons: &BTreeMap<String, AppraisalLexiconSection>,
) -> Vec<AppraisalEvent> {
    let lower = text.to_lowercase();
    let mut events = Vec::new();

    let has_positive_phrase =
        lexicon_contains_any(lexicons, |lexicon| &lexicon.positive_phrases, &lower);
    let has_correction_or_failure =
        lexicon_contains_any(lexicons, |lexicon| &lexicon.correction_or_failure, &lower);
    let sarcasm_marker = lexicon_contains_any(lexicons, |lexicon| &lexicon.sarcasm_markers, &lower);
    if (has_positive_phrase && has_correction_or_failure)
        || (sarcasm_marker && has_correction_or_failure)
    {
        events.push(appraisal(
            "user_sarcasm_or_dissatisfaction",
            AppraisalSource::Classifier,
            AppraisalTarget::Dialogue,
            0.80,
            AppraisalScope::Session,
            now,
            policy,
            AffectDeltas {
                valence: -0.10,
                arousal: 0.06,
                calmness: -0.06,
                engagement: 0.02,
                ..AffectDeltas::default()
            },
            "Classifier detected sarcasm or dissatisfied praise",
        ));
        events.push(appraisal(
            "relationship_repair_needed",
            AppraisalSource::Classifier,
            AppraisalTarget::Dialogue,
            0.70,
            AppraisalScope::User,
            now,
            policy,
            AffectDeltas {
                trust: -0.014,
                familiarity: 0.004,
                ..AffectDeltas::default()
            },
            "Sarcasm/dissatisfaction adds a small relationship repair signal",
        ));
    }

    if lexicon_contains_any(lexicons, |lexicon| &lexicon.support_need, &lower) {
        events.push(appraisal(
            "user_support_need",
            AppraisalSource::Classifier,
            AppraisalTarget::User,
            0.78,
            AppraisalScope::Session,
            now,
            policy,
            AffectDeltas {
                valence: -0.08,
                arousal: 0.04,
                calmness: -0.05,
                engagement: 0.05,
                ..AffectDeltas::default()
            },
            "Classifier detected user distress or need for support",
        ));
    }

    if lexicon_contains_any(lexicons, |lexicon| &lexicon.safety_salience, &lower) {
        events.push(appraisal(
            "safety_salience",
            AppraisalSource::Classifier,
            AppraisalTarget::Dialogue,
            0.82,
            AppraisalScope::Turn,
            now,
            policy,
            AffectDeltas {
                arousal: 0.06,
                calmness: -0.04,
                engagement: 0.04,
                ..AffectDeltas::default()
            },
            "Classifier detected safety-relevant user content",
        ));
    }

    if lexicon_contains_any(lexicons, |lexicon| &lexicon.minimal_expression, &lower) {
        events.push(appraisal(
            "user_requested_minimal_expression",
            AppraisalSource::Classifier,
            AppraisalTarget::Dialogue,
            0.76,
            AppraisalScope::Session,
            now,
            policy,
            AffectDeltas {
                arousal: -0.03,
                interaction_load: -0.04,
                ..AffectDeltas::default()
            },
            "Classifier detected user preference for minimal expression",
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

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn lexicon_contains_any(
    lexicons: &BTreeMap<String, AppraisalLexiconSection>,
    selector: impl Fn(&AppraisalLexiconSection) -> &[String],
    haystack: &str,
) -> bool {
    lexicons
        .values()
        .any(|lexicon| contains_any_owned(haystack, selector(lexicon)))
}

fn contains_any_owned(haystack: &str, needles: &[String]) -> bool {
    needles
        .iter()
        .any(|needle| haystack.contains(&needle.to_lowercase()))
}
