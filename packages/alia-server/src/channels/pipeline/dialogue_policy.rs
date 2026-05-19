use crate::config::DialoguePolicyResourceSection;
use crate::expression::{ExpressionConstraints, ExpressionConversationContext};

#[derive(Debug, Clone)]
pub(super) struct DialoguePolicyInput<'a> {
    pub route: &'a str,
    pub used_tools: bool,
    pub response_was_fallback: bool,
    pub cache_hit: bool,
    pub user_text: &'a str,
    pub memory_context_used: bool,
}

#[derive(Debug, Clone)]
pub(super) struct DialoguePolicyDecision {
    pub dialogue_act: String,
    pub content_tone: String,
    pub turn_kind: String,
    pub prefer_minimal: bool,
    pub require_clarity: bool,
    pub allow_playful: bool,
    pub max_intensity: Option<f32>,
    pub max_motion_intensity: Option<f32>,
}

impl DialoguePolicyDecision {
    pub(super) fn conversation_context(
        &self,
        input: &DialoguePolicyInput<'_>,
        prior_message_count: usize,
    ) -> ExpressionConversationContext {
        ExpressionConversationContext {
            turn_kind: self.turn_kind.clone(),
            route: Some(input.route.to_string()),
            used_tools: input.used_tools,
            response_was_fallback: input.response_was_fallback,
            cache_hit: input.cache_hit,
            prior_message_count,
        }
    }

    pub(super) fn constraints(&self) -> ExpressionConstraints {
        ExpressionConstraints {
            prefer_minimal: self.prefer_minimal,
            require_clarity: self.require_clarity,
            allow_playful: self.allow_playful,
            max_intensity: self.max_intensity,
            max_motion_intensity: self.max_motion_intensity,
        }
    }
}

pub(super) fn decide(
    input: DialoguePolicyInput<'_>,
    policy: &DialoguePolicyResourceSection,
) -> DialoguePolicyDecision {
    let mut decision = DialoguePolicyDecision {
        dialogue_act: "respond".to_string(),
        content_tone: "neutral".to_string(),
        turn_kind: "assistant_response".to_string(),
        prefer_minimal: input.cache_hit,
        require_clarity: input.used_tools || matches!(input.route, "tool" | "tool_call"),
        allow_playful: !input.response_was_fallback && !input.used_tools,
        max_intensity: input.response_was_fallback.then_some(0.32),
        max_motion_intensity: if input.response_was_fallback || input.cache_hit {
            Some(0.0)
        } else if input.used_tools || matches!(input.route, "tool" | "tool_call") {
            Some(0.25)
        } else {
            None
        },
    };

    if input.response_was_fallback {
        decision.dialogue_act = "apologize".to_string();
        decision.content_tone = "serious".to_string();
        decision.turn_kind = "fallback_response".to_string();
        decision.allow_playful = false;
        return decision;
    }

    if input.cache_hit || matches!(input.route, "cache" | "router_cache") {
        decision.dialogue_act = "focus".to_string();
        decision.content_tone = "concise".to_string();
        decision.turn_kind = "cached_response".to_string();
        decision.allow_playful = false;
        return decision;
    }

    let lower = input.user_text.to_lowercase();
    let asks_question = policy
        .question_suffixes
        .iter()
        .any(|suffix| input.user_text.ends_with(suffix));
    let frustrated = contains_any_text(&lower, &policy.frustration_phrases);

    if input.used_tools || matches!(input.route, "tool" | "tool_call" | "skill") {
        decision.dialogue_act = "focus".to_string();
        decision.content_tone = "technical".to_string();
        decision.turn_kind = "tool_assisted_response".to_string();
    } else if frustrated {
        decision.dialogue_act = "apologize".to_string();
        decision.content_tone = "serious".to_string();
        decision.turn_kind = "repair_response".to_string();
        decision.allow_playful = false;
        decision.max_intensity = Some(decision.max_intensity.unwrap_or(0.34).min(0.34));
    } else if contains_any_text(&lower, &policy.safety_phrases) {
        decision.dialogue_act = "warn".to_string();
        decision.content_tone = "safety".to_string();
        decision.turn_kind = "safety_response".to_string();
        decision.allow_playful = false;
    } else if contains_any_text(&lower, &policy.support_need_phrases) {
        decision.dialogue_act = "comfort".to_string();
        decision.content_tone = "supportive".to_string();
        decision.turn_kind = "support_response".to_string();
        decision.allow_playful = false;
    } else if contains_any_text(&lower, &policy.celebration_phrases) {
        decision.dialogue_act = "celebrate".to_string();
        decision.content_tone = "playful".to_string();
        decision.turn_kind = "celebration_response".to_string();
    } else if asks_question || contains_any_text(&lower, &policy.explanation_phrases) {
        decision.dialogue_act = "explain".to_string();
        decision.content_tone = if contains_any_text(&lower, &policy.technical_phrases) {
            "technical".to_string()
        } else {
            "neutral".to_string()
        };
        decision.turn_kind = "explanation_response".to_string();
    }

    if input.memory_context_used {
        decision.require_clarity = true;
        decision.content_tone = if decision.content_tone == "neutral" {
            "precise".to_string()
        } else {
            decision.content_tone
        };
    }

    decision
}

fn contains_any_text(haystack: &str, needles: &[String]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_user_semantics() {
        let decision = decide(
            DialoguePolicyInput {
                route: "major_llm",
                used_tools: false,
                response_was_fallback: false,
                cache_hit: false,
                user_text: "I feel sad and could use some comfort.",
                memory_context_used: false,
            },
            &DialoguePolicyResourceSection::default(),
        );

        assert_eq!(decision.dialogue_act, "comfort");
        assert_eq!(decision.content_tone, "supportive");
        assert_eq!(decision.turn_kind, "support_response");
        assert!(!decision.allow_playful);
    }

    #[test]
    fn preserves_cache_minimality() {
        let input = DialoguePolicyInput {
            route: "cache",
            used_tools: false,
            response_was_fallback: false,
            cache_hit: true,
            user_text: "How do I configure the API?",
            memory_context_used: false,
        };
        let decision = decide(input, &DialoguePolicyResourceSection::default());

        assert_eq!(decision.dialogue_act, "focus");
        assert_eq!(decision.content_tone, "concise");
        assert!(decision.prefer_minimal);
        assert_eq!(decision.max_motion_intensity, Some(0.0));
    }

    #[test]
    fn uses_configured_phrase_lists() {
        let mut policy = DialoguePolicyResourceSection::default();
        policy.support_need_phrases = vec!["heavy heart".to_string()];

        let decision = decide(
            DialoguePolicyInput {
                route: "major_llm",
                used_tools: false,
                response_was_fallback: false,
                cache_hit: false,
                user_text: "I have a heavy heart today and want to talk slowly.",
                memory_context_used: false,
            },
            &policy,
        );

        assert_eq!(decision.dialogue_act, "comfort");
        assert_eq!(decision.content_tone, "supportive");
    }
}
