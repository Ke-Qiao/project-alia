//! Multi-layer router backed by the configured minor LLM.
//!
//! The router asks the minor model to classify the current turn, then routes to
//! a skill context, the minor LLM, or the major LLM. It is intentionally
//! provider-neutral: the minor model is supplied through `LlmAdapter`, so the
//! server does not depend on a specific local model runtime.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;
use tokio::time::timeout;

use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{
    ChatMessage, LlmAdapter, RouteDecision, RouterInput, RouterStrategy, ToolSelectionInput,
};
use crate::settings::values::{bool_setting, u64_setting};

#[derive(Debug, Deserialize)]
pub struct Classification {
    pub route: String,
    #[serde(default)]
    pub skill_name: Option<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolSelectionResponse {
    #[serde(default)]
    selected: Vec<String>,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RouterConfig {
    pub timeout_sec: u64,
    pub minor_llm_enabled: bool,
    pub major_llm_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmMode {
    BothEnabled,
    MinorOnly,
    MajorOnly,
}

impl RouterConfig {
    pub fn llm_mode(&self) -> LlmMode {
        match (self.minor_llm_enabled, self.major_llm_enabled) {
            (true, true) => LlmMode::BothEnabled,
            (true, false) => LlmMode::MinorOnly,
            (false, true) => LlmMode::MajorOnly,
            (false, false) => LlmMode::MajorOnly,
        }
    }
}

#[derive(Clone)]
struct RouterState {
    config: RouterConfig,
    minor_llm: Option<Arc<dyn LlmAdapter>>,
}

/// Classification prompt when both LLM tiers are available.
const CLASSIFICATION_PROMPT_BOTH: &str = r#"You are an intent classifier inside a smart home AI assistant called "Alia". You do NOT answer the user. Your only job is to classify the user's message into one of three execution routes so the correct downstream system handles it.

## Your output

Respond with a single JSON object. No markdown fences, no explanation, no extra text.

```
{"route":"<route>","skill_name":"<if skill>","confidence":<0-1>,"reasoning":"<one line>"}
```

## Routes

### "skill" - deterministic device/system commands
Use when the user gives a clear, actionable command that maps to one of these skills:

| skill_name | When to use |
|---|---|
| smart_light_control | Turn on/off lights, set brightness/color temp |
| smart_ac_control | AC on/off, set temperature/mode |
| smart_appliance | Curtains, fans, plugs, TV, vacuum, etc. |
| timer_alarm | Set timer, countdown, alarm, reminder |
| weather_query | Current weather, forecast, rain/snow check |
| device_status | "Is the light on?", "What's the temperature?" |
| memory_query | "Do you remember...", "What did I say last time?" |
| rule_management | Create/edit/delete automation rules |

Do not extract action parameters here. Skills are cognitive context only; downstream tool calling will produce strict tool arguments.

### "minor_llm" - simple, short replies
Use when a lightweight model can handle it in one turn:
- Greetings ("Hi", "Good morning"), farewells, thanks, acknowledgments
- Yes/no confirmations, simple follow-ups ("OK", "got it", "and then?")
- Single-fact questions with short answers ("What day is it?", "What's 2+2?")
- Casual chit-chat that does not reference user history or need personalization

### "major_llm" - complex reasoning, personalization, or uncertainty
Use for anything that needs deep thinking, creativity, or user context:
- Multi-step reasoning, analysis, comparisons, advice
- Creative writing, stories, code generation
- Questions about user preferences, past conversations, or personalized responses
- Long or nuanced conversations, emotional support
- Ambiguous requests that need interpretation
- Anything referencing prior context deeply ("based on what we discussed...")

## Decision priority

1. If the message is a clear device/system command, choose "skill".
2. If it is trivial small-talk or a one-word/simple reply, choose "minor_llm".
3. Everything else chooses "major_llm".

When in doubt between "minor_llm" and "major_llm", always choose "major_llm".
When in doubt between "skill" and "major_llm", choose "major_llm" so the downstream LLM can reason with tools."#;

/// Classification prompt when only the minor LLM tier is available.
const CLASSIFICATION_PROMPT_MINOR_ONLY: &str = r#"You are an intent classifier inside a smart home AI assistant called "Alia". You do NOT answer the user. Your only job is to classify the user's message into one of two execution routes so the correct downstream system handles it.

## Your output

Respond with a single JSON object. No markdown fences, no explanation, no extra text.

```
{"route":"<route>","skill_name":"<if skill>","confidence":<0-1>,"reasoning":"<one line>"}
```

## Routes

### "skill" - deterministic device/system commands
Use when the user gives a clear, actionable command that maps to one of these skills:

| skill_name | When to use |
|---|---|
| smart_light_control | Turn on/off lights, set brightness/color temp |
| smart_ac_control | AC on/off, set temperature/mode |
| smart_appliance | Curtains, fans, plugs, TV, vacuum, etc. |
| timer_alarm | Set timer, countdown, alarm, reminder |
| weather_query | Current weather, forecast, rain/snow check |
| device_status | "Is the light on?", "What's the temperature?" |
| memory_query | "Do you remember...", "What did I say last time?" |
| rule_management | Create/edit/delete automation rules |

Do not extract action parameters here. Skills are cognitive context only; downstream tool calling will produce strict tool arguments.

### "minor_llm" - all other messages
Route everything that is not a skill command to the minor LLM. This is the only general-purpose model available.

## Decision priority

1. If the message is a clear device/system command, choose "skill".
2. Everything else chooses "minor_llm"."#;

/// System prompt given to the minor LLM when both tiers are available.
const MINOR_LLM_SYSTEM_PROMPT: &str = r#"You are Alia, a friendly and concise smart home AI assistant running on the minor model. You handle simple conversations: greetings, casual chat, quick factual answers, and short confirmations.

Guidelines:
- Keep responses brief (1-3 sentences).
- Be warm and natural, not robotic.
- Do NOT attempt complex reasoning, creative writing, code generation, or multi-step analysis.
- If the user's request seems too complex, requires deep personalization, or involves long context reasoning, respond with exactly: [ESCALATE] followed by a one-sentence explanation of why.
  Example: "[ESCALATE] This question requires analyzing past context and preferences."
- Do not perform deep memory or preference reasoning. If the answer depends on memory, preferences, or conversation history beyond the visible system context, escalate.
- If tools are available, use tool calls instead of writing tool JSON or hidden arguments.
- Answer in the same language the user uses."#;

/// System prompt given to the minor LLM when it is the only available tier.
const MINOR_LLM_SYSTEM_PROMPT_SOLO: &str = r#"You are Alia, a friendly and helpful smart home AI assistant. You are the primary and only language model, so you handle all conversations - from simple greetings to complex reasoning.

Guidelines:
- Be warm, natural, and helpful.
- Adapt response length to the complexity of the question.
- Answer in the same language the user uses.
- For complex tasks, think step by step.
- You may not have full conversation history, so ask for clarification when needed.
- If tools are available, use tool calls instead of writing tool JSON or hidden arguments."#;

/// System prompt given to the major LLM when handling routed complex turns.
const MAJOR_LLM_SYSTEM_PROMPT: &str = r#"You are Alia, an intelligent and empathetic smart home AI assistant. You are the primary reasoning model, handling complex conversations that require deep thinking, creativity, personalization, or multi-step analysis.

Guidelines:
- Be warm, thoughtful, and personalized. Use the user's memory and context when available.
- Adapt response length and depth to the question - short for simple follow-ups, thorough for complex topics.
- Answer in the same language the user uses.
- You have access to the user's memory, preferences, and conversation history via the system context.
- For creative tasks, be expressive. For analytical tasks, be structured and clear.
- When a skill context is active, use the tools provided by that context. Do not call skills directly.
- Never expose raw tool-call JSON, function names, or hidden arguments in the final answer."#;

/// System prompt given to the major LLM when it is the only available tier.
const MAJOR_LLM_SYSTEM_PROMPT_SOLO: &str = r#"You are Alia, an intelligent and empathetic smart home AI assistant. You handle all conversations - from casual greetings to complex reasoning and creative tasks.

Guidelines:
- Be warm, natural, and personalized. Use the user's memory and context when available.
- Keep simple responses brief (1-2 sentences). Give thorough answers for complex topics.
- Answer in the same language the user uses.
- You have access to the user's memory, preferences, and conversation history via the system context.
- For creative tasks, be expressive. For analytical tasks, be structured and clear.
- When a skill context is active, use the tools provided by that context. Do not call skills directly.
- Never expose raw tool-call JSON, function names, or hidden arguments in the final answer."#;

const TOOL_SELECTION_PROMPT: &str = r#"You are a fast tool candidate selector inside Alia.
You do NOT answer the user. Select only tools that are likely useful for this exact user request.

Rules:
- Return one JSON object only: {"selected":["tool_name", "..."], "reasoning":"short"}
- Select at most max_tools names.
- Use only names from the candidate list.
- Prefer precise tools over broad tools.
- If no tool is needed, return {"selected":[]}.
- Skills are cognitive context, not callable tools.
"#;

fn classification_prompt_for_mode(mode: LlmMode) -> Option<&'static str> {
    match mode {
        LlmMode::BothEnabled => Some(CLASSIFICATION_PROMPT_BOTH),
        LlmMode::MinorOnly => Some(CLASSIFICATION_PROMPT_MINOR_ONLY),
        LlmMode::MajorOnly => None,
    }
}

#[derive(Default)]
pub struct MultiLayerRouter {
    state: Arc<RwLock<Option<RouterState>>>,
}

impl MultiLayerRouter {
    pub fn new() -> Self {
        Self::default()
    }

    async fn classify(
        minor_llm: Arc<dyn LlmAdapter>,
        config: &RouterConfig,
        input: &RouterInput,
    ) -> anyhow::Result<Classification> {
        let mode = config.llm_mode();
        let Some(prompt) = classification_prompt_for_mode(mode) else {
            return Ok(Classification {
                route: "major_llm".to_string(),
                skill_name: None,
                confidence: Some(1.0),
                reasoning: Some("major-only mode, skipping classification".to_string()),
            });
        };

        let mut messages = vec![ChatMessage::new("system", prompt)];
        let context_window = input.conversation_context.len().min(4);
        for message in input
            .conversation_context
            .iter()
            .rev()
            .take(context_window)
            .rev()
        {
            messages.push(message.clone());
        }
        messages.push(ChatMessage::new("user", input.text.clone()));

        let raw = timeout(
            Duration::from_secs(config.timeout_sec),
            minor_llm.chat(&messages),
        )
        .await
        .map_err(|_| anyhow::anyhow!("router classification timed out"))??;

        let classification: Classification = serde_json::from_str(strip_code_fences(&raw))
            .map_err(|error| {
                tracing::warn!(
                    raw_response = %raw,
                    error = %error,
                    "failed to parse router classification"
                );
                anyhow::anyhow!("classification parse error: {error}")
            })?;

        tracing::debug!(
            route = %classification.route,
            skill = ?classification.skill_name,
            confidence = ?classification.confidence,
            reasoning = ?classification.reasoning,
            "router classification result"
        );
        Ok(classification)
    }

    async fn select_tools_with_minor_model(
        minor_llm: Arc<dyn LlmAdapter>,
        config: &RouterConfig,
        input: &ToolSelectionInput,
    ) -> anyhow::Result<Option<Vec<String>>> {
        if matches!(config.llm_mode(), LlmMode::MajorOnly) {
            return Ok(None);
        }
        if input.candidates.is_empty() || input.max_tools == 0 {
            return Ok(Some(Vec::new()));
        }

        let candidate_names = input
            .candidates
            .iter()
            .map(|candidate| candidate.name.as_str())
            .collect::<Vec<_>>();
        let payload = serde_json::json!({
            "user_text": &input.text,
            "channel_id": &input.channel_id,
            "max_tools": input.max_tools,
            "candidates": &input.candidates,
        });
        let messages = vec![
            ChatMessage::new("system", TOOL_SELECTION_PROMPT),
            ChatMessage::new("user", payload.to_string()),
        ];

        let raw = timeout(
            Duration::from_secs(config.timeout_sec),
            minor_llm.chat(&messages),
        )
        .await
        .map_err(|_| anyhow::anyhow!("minor tool selection timed out"))??;
        let selected = parse_tool_selection_response(&raw, &candidate_names, input.max_tools)?;
        tracing::debug!(selected = ?selected, "minor LLM tool candidate selection result");
        Ok(Some(selected))
    }

    pub fn to_decision(classification: Classification, mode: LlmMode) -> RouteDecision {
        match classification.route.trim().to_ascii_lowercase().as_str() {
            "skill" => {
                let Some(skill_name) = classification
                    .skill_name
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                else {
                    return Self::fallback_decision(mode);
                };
                RouteDecision::Skill { skill_name }
            }
            "minor_llm" => {
                let prompt = match mode {
                    LlmMode::MinorOnly => MINOR_LLM_SYSTEM_PROMPT_SOLO,
                    _ => MINOR_LLM_SYSTEM_PROMPT,
                };
                let escalation_prompt = match mode {
                    LlmMode::BothEnabled => Some(MAJOR_LLM_SYSTEM_PROMPT.to_string()),
                    _ => None,
                };
                RouteDecision::MinorLlm {
                    system_prompt: Some(prompt.to_string()),
                    escalation_prompt,
                }
            }
            _ => Self::fallback_decision(mode),
        }
    }

    fn fallback_decision(mode: LlmMode) -> RouteDecision {
        match mode {
            LlmMode::MinorOnly => RouteDecision::MinorLlm {
                system_prompt: Some(MINOR_LLM_SYSTEM_PROMPT_SOLO.to_string()),
                escalation_prompt: None,
            },
            LlmMode::MajorOnly => RouteDecision::MajorLlm {
                system_prompt: Some(MAJOR_LLM_SYSTEM_PROMPT_SOLO.to_string()),
            },
            LlmMode::BothEnabled => RouteDecision::MajorLlm {
                system_prompt: Some(MAJOR_LLM_SYSTEM_PROMPT.to_string()),
            },
        }
    }
}

#[async_trait]
impl ModuleLifecycle for MultiLayerRouter {
    fn name(&self) -> &'static str {
        "multi_layer"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let routing = &ctx.config.policies.routing;
        if !routing.minor_llm_enabled && !routing.major_llm_enabled {
            anyhow::bail!(
                "invalid routing policy: at least one of minor_llm_enabled or major_llm_enabled must be true"
            );
        }

        let config = RouterConfig {
            timeout_sec: routing.timeout_sec,
            minor_llm_enabled: routing.minor_llm_enabled,
            major_llm_enabled: routing.major_llm_enabled,
        };
        tracing::info!(
            timeout_sec = config.timeout_sec,
            minor_llm = config.minor_llm_enabled,
            major_llm = config.major_llm_enabled,
            mode = ?config.llm_mode(),
            "MultiLayerRouter initialized"
        );

        *self.state.write().await = Some(RouterState {
            config,
            minor_llm: ctx.minor_llm.clone(),
        });
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let state = self.state.read().await;
        let Some(state) = state.as_ref() else {
            return ModuleHealth::Unhealthy("not initialized".to_string());
        };
        if matches!(state.config.llm_mode(), LlmMode::MajorOnly) {
            return ModuleHealth::Healthy;
        }
        let Some(minor_llm) = &state.minor_llm else {
            return ModuleHealth::Degraded("minor LLM unavailable; routing falls back".to_string());
        };
        match minor_llm.health_check().await {
            ModuleHealth::Healthy => ModuleHealth::Healthy,
            other => other,
        }
    }

    async fn apply_settings(
        &self,
        updates: &BTreeMap<String, Value>,
    ) -> anyhow::Result<Vec<String>> {
        let mut state_guard = self.state.write().await;
        let Some(state) = state_guard.as_mut() else {
            return Ok(Vec::new());
        };

        let mut next = state.config.clone();
        let mut applied = Vec::new();
        if let Some(value) = u64_setting(updates, "core.policies.routing.timeout_sec") {
            next.timeout_sec = value;
            applied.push("core.policies.routing.timeout_sec".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.routing.minor_llm_enabled") {
            next.minor_llm_enabled = value;
            applied.push("core.policies.routing.minor_llm_enabled".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.routing.major_llm_enabled") {
            next.major_llm_enabled = value;
            applied.push("core.policies.routing.major_llm_enabled".to_string());
        }
        if !next.minor_llm_enabled && !next.major_llm_enabled {
            anyhow::bail!(
                "invalid routing policy: at least one of minor_llm_enabled or major_llm_enabled must stay enabled"
            );
        }
        if !applied.is_empty() {
            state.config = next;
        }
        Ok(applied)
    }
}

#[async_trait]
impl RouterStrategy for MultiLayerRouter {
    async fn route(&self, input: &RouterInput) -> anyhow::Result<RouteDecision> {
        let state = self
            .state
            .read()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("router not initialized"))?;
        let mode = state.config.llm_mode();

        let Some(minor_llm) = state.minor_llm else {
            return Ok(Self::fallback_decision(mode));
        };

        match Self::classify(minor_llm, &state.config, input).await {
            Ok(classification) => Ok(Self::to_decision(classification, mode)),
            Err(error) => {
                tracing::warn!(error = %error, "router classification failed, falling back");
                Ok(Self::fallback_decision(mode))
            }
        }
    }

    async fn select_tool_candidates(
        &self,
        input: &ToolSelectionInput,
    ) -> anyhow::Result<Option<Vec<String>>> {
        let state = self
            .state
            .read()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("router not initialized"))?;
        let Some(minor_llm) = state.minor_llm else {
            return Ok(None);
        };
        Self::select_tools_with_minor_model(minor_llm, &state.config, input).await
    }
}

pub fn strip_code_fences(s: &str) -> &str {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = if let Some(nl) = rest.find('\n') {
            &rest[nl + 1..]
        } else {
            rest
        };
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    }
}

pub fn parse_tool_selection_response(
    raw: &str,
    candidate_names: &[&str],
    max_tools: usize,
) -> anyhow::Result<Vec<String>> {
    let candidate_names = candidate_names.iter().copied().collect::<HashSet<_>>();
    let selection: ToolSelectionResponse = serde_json::from_str(strip_code_fences(raw))
        .map_err(|e| anyhow::anyhow!("tool selection parse error: {e}"))?;

    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for name in selection.selected {
        if selected.len() >= max_tools {
            break;
        }
        if candidate_names.contains(name.as_str()) && seen.insert(name.clone()) {
            selected.push(name);
        }
    }
    tracing::debug!(reasoning = ?selection.reasoning, "parsed minor tool selection");
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_mode_classification_prompt_preserves_route_boundaries() {
        let prompt = classification_prompt_for_mode(LlmMode::BothEnabled).unwrap();

        assert!(prompt.contains("Respond with a single JSON object"));
        assert!(prompt.contains("No markdown fences"));
        assert!(prompt.contains(r#""skill""#));
        assert!(prompt.contains(r#""minor_llm""#));
        assert!(prompt.contains(r#""major_llm""#));
        assert!(prompt.contains("smart_light_control"));
        assert!(prompt.contains("Do not extract action parameters here"));
        assert!(prompt.contains("When in doubt between \"minor_llm\" and \"major_llm\""));
        assert!(prompt.contains("always choose \"major_llm\""));
    }

    #[test]
    fn minor_only_classification_prompt_removes_major_route() {
        let prompt = classification_prompt_for_mode(LlmMode::MinorOnly).unwrap();

        assert!(prompt.contains(r#""skill""#));
        assert!(prompt.contains(r#""minor_llm""#));
        assert!(!prompt.contains("### \"major_llm\""));
        assert!(prompt.contains("This is the only general-purpose model available"));
        assert!(prompt.contains("Everything else chooses \"minor_llm\""));
    }

    #[test]
    fn major_only_mode_skips_classification_prompt() {
        assert!(classification_prompt_for_mode(LlmMode::MajorOnly).is_none());
    }

    #[test]
    fn conversation_prompts_preserve_escalation_and_tool_rules() {
        assert!(MINOR_LLM_SYSTEM_PROMPT.contains("[ESCALATE]"));
        assert!(MINOR_LLM_SYSTEM_PROMPT.contains("deep memory or preference reasoning"));
        assert!(MINOR_LLM_SYSTEM_PROMPT_SOLO.contains("primary and only language model"));
        assert!(MAJOR_LLM_SYSTEM_PROMPT.contains("primary reasoning model"));
        assert!(MAJOR_LLM_SYSTEM_PROMPT.contains("Do not call skills directly"));
        assert!(MAJOR_LLM_SYSTEM_PROMPT_SOLO.contains("all conversations"));
        assert!(TOOL_SELECTION_PROMPT.contains("Select only tools that are likely useful"));
        assert!(TOOL_SELECTION_PROMPT.contains("Skills are cognitive context, not callable tools"));
    }
}
