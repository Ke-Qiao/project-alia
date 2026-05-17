//! MultiLayerRouter: Intent classification via local Ollama model.
//!
//! Uses a lightweight local model (e.g. Qwen3-4B) to classify user intent,
//! then routes to the appropriate execution path:
//!   - SKILL → matched skill context and bound tools
//!   - LOCAL_LLM → simple chat handled by local Ollama model
//!   - REMOTE_LLM → complex reasoning forwarded to remote LLM (GPT-4o / Claude)

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{RouteDecision, RouterInput, RouterStrategy, ToolSelectionInput};

// ── Ollama API types (local to this module) ────────────────────────────

#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    temperature: f32,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    message: OllamaMessage,
    #[allow(dead_code)]
    done: bool,
}

#[derive(Debug, Deserialize)]
struct ToolSelectionResponse {
    #[serde(default)]
    selected: Vec<String>,
    #[serde(default)]
    reasoning: Option<String>,
}

// ── Classification result ──────────────────────────────────────────────

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

// ── Config ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RouterConfig {
    pub ollama_endpoint: String,
    pub model: String,
    pub temperature: f32,
    pub timeout_sec: u64,
    pub local_llm_enabled: bool,
    pub remote_llm_enabled: bool,
}

/// Which LLM tiers are available for routing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmMode {
    BothEnabled,
    LocalOnly,
    RemoteOnly,
}

impl RouterConfig {
    pub fn llm_mode(&self) -> LlmMode {
        match (self.local_llm_enabled, self.remote_llm_enabled) {
            (true, true) => LlmMode::BothEnabled,
            (true, false) => LlmMode::LocalOnly,
            (false, true) => LlmMode::RemoteOnly,
            // Should be caught at init, but fall back to remote
            (false, false) => LlmMode::RemoteOnly,
        }
    }
}

// ── System prompts ─────────────────────────────────────────────────────

/// Classification prompt when both LLMs are available
const CLASSIFICATION_PROMPT_BOTH: &str = r#"You are an intent classifier inside a smart home AI assistant called "Alia". You do NOT answer the user. Your only job is to classify the user's message into one of three execution routes so the correct downstream system handles it.

## Your output

Respond with a single JSON object. No markdown fences, no explanation, no extra text.

```
{"route":"<route>","skill_name":"<if skill>","confidence":<0-1>,"reasoning":"<one line>"}
```

## Routes

### "skill" — deterministic device/system commands
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

### "local_llm" — simple, short replies
Use when a lightweight model can handle it in one turn:
- Greetings ("Hi", "Good morning"), farewells, thanks, acknowledgments
- Yes/no confirmations, simple follow-ups ("OK", "got it", "and then?")
- Single-fact questions with short answers ("What day is it?", "What's 2+2?")
- Casual chit-chat that doesn't reference user history or need personalization

### "remote_llm" — complex reasoning (default when uncertain)
Use for anything that needs deep thinking, creativity, or user context:
- Multi-step reasoning, analysis, comparisons, advice
- Creative writing, stories, code generation
- Questions about user preferences, past conversations, personalized responses
- Long or nuanced conversations, emotional support
- Ambiguous requests that need interpretation
- Anything referencing prior context deeply ("based on what we discussed...")

## Decision priority

1. If the message is a clear device/system command → "skill"
2. If it's trivial small-talk or a one-word reply → "local_llm"
3. Everything else → "remote_llm"

When in doubt between "local_llm" and "remote_llm", always choose "remote_llm".
When in doubt between "skill" and "remote_llm", choose "remote_llm" (let the LLM figure it out with tool use)."#;

/// Classification prompt when only local LLM is available (no remote option)
const CLASSIFICATION_PROMPT_LOCAL_ONLY: &str = r#"You are an intent classifier inside a smart home AI assistant called "Alia". You do NOT answer the user. Your only job is to classify the user's message into one of two execution routes so the correct downstream system handles it.

## Your output

Respond with a single JSON object. No markdown fences, no explanation, no extra text.

```
{"route":"<route>","skill_name":"<if skill>","confidence":<0-1>,"reasoning":"<one line>"}
```

## Routes

### "skill" — deterministic device/system commands
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

### "local_llm" — all other messages
Route everything that is not a skill command to the local LLM. This is the only general-purpose model available.

## Decision priority

1. If the message is a clear device/system command → "skill"
2. Everything else → "local_llm""#;

/// System prompt given to the **local LLM** when it handles a conversation (both-mode).
/// Tells it to stay in scope and escalate complex tasks.
const LOCAL_LLM_SYSTEM_PROMPT: &str = r#"You are Alia, a friendly and concise smart home AI assistant running on a local model. You handle simple conversations: greetings, casual chat, quick factual answers, and short confirmations.

Guidelines:
- Keep responses brief (1-3 sentences).
- Be warm and natural, not robotic.
- Do NOT attempt complex reasoning, creative writing, code generation, or multi-step analysis.
- If the user's request seems too complex, requires deep personalization, or involves long context reasoning, respond with exactly: [ESCALATE] followed by a one-sentence explanation of why.
  Example: "[ESCALATE] This question requires analyzing your past conversations and preferences."
- You do NOT have access to the user's memory, preferences, or conversation history. If asked about those, escalate.
- If tools are available, use tool calls instead of writing tool JSON or internal arguments in the answer.
- Answer in the same language the user uses."#;

/// System prompt given to the **local LLM** when it's the only model (local-only mode).
/// No escalation — it handles everything.
const LOCAL_LLM_SYSTEM_PROMPT_SOLO: &str = r#"You are Alia, a friendly and helpful smart home AI assistant. You are the primary and only language model, so you handle all conversations — from simple greetings to complex reasoning.

Guidelines:
- Be warm, natural, and helpful.
- Adapt response length to the complexity of the question.
- Answer in the same language the user uses.
- For complex tasks, think step by step.
- You may not have full conversation history, so ask for clarification when needed.
- If tools are available, use tool calls instead of writing tool JSON or internal arguments in the answer."#;

/// System prompt given to the **remote LLM** when handling a conversation.
const REMOTE_LLM_SYSTEM_PROMPT: &str = r#"You are Alia, an intelligent and empathetic smart home AI assistant. You are the primary reasoning model, handling complex conversations that require deep thinking, creativity, personalization, or multi-step analysis.

Guidelines:
- Be warm, thoughtful, and personalized. Use the user's memory and context when available.
- Adapt response length and depth to the question — short for simple follow-ups, thorough for complex topics.
- Answer in the same language the user uses.
- You have access to the user's memory, preferences, and conversation history via the system context.
- For creative tasks, be expressive. For analytical tasks, be structured and clear.
- When a skill context is active, use the tools provided by that context. Do not call skills directly.
- Never expose raw tool-call JSON, function names, or hidden arguments in the final answer."#;

/// System prompt given to the **remote LLM** when it's the only model (remote-only mode).
const REMOTE_LLM_SYSTEM_PROMPT_SOLO: &str = r#"You are Alia, an intelligent and empathetic smart home AI assistant. You handle all conversations — from casual greetings to complex reasoning and creative tasks.

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

// ── Implementation ─────────────────────────────────────────────────────

pub struct MultiLayerRouter {
    client: Client,
    config: Arc<RwLock<Option<RouterConfig>>>,
}

impl MultiLayerRouter {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            config: Arc::new(RwLock::new(None)),
        }
    }

    /// Call the local Ollama model for intent classification.
    async fn classify(
        &self,
        config: &RouterConfig,
        input: &RouterInput,
    ) -> anyhow::Result<Classification> {
        let mode = config.llm_mode();

        // Pick classification prompt based on available LLM tiers
        let classification_prompt = match mode {
            LlmMode::RemoteOnly => {
                // No local LLM — skip classification entirely, always remote
                return Ok(Classification {
                    route: "remote_llm".to_string(),
                    skill_name: None,
                    confidence: Some(1.0),
                    reasoning: Some("remote-only mode, skipping classification".to_string()),
                });
            }
            LlmMode::LocalOnly => CLASSIFICATION_PROMPT_LOCAL_ONLY,
            LlmMode::BothEnabled => CLASSIFICATION_PROMPT_BOTH,
        };

        // Build messages: system prompt + conversation context (last few) + user message
        let mut messages = vec![OllamaMessage {
            role: "system".to_string(),
            content: classification_prompt.to_string(),
        }];

        // Include recent conversation context (last 4 turns max) for better classification
        let context_window = input.conversation_context.len().min(4);
        for msg in input
            .conversation_context
            .iter()
            .rev()
            .take(context_window)
            .rev()
        {
            messages.push(OllamaMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
            });
        }

        // Current user message
        messages.push(OllamaMessage {
            role: "user".to_string(),
            content: input.text.clone(),
        });

        let request = OllamaRequest {
            model: config.model.clone(),
            messages,
            stream: false,
            options: Some(OllamaOptions {
                temperature: config.temperature,
            }),
        };

        let url = format!("{}/api/chat", config.ollama_endpoint);
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ollama request failed: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("ollama returned {status}: {body}");
        }

        let ollama_resp: OllamaResponse = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("ollama response parse failed: {e}"))?;

        let raw = ollama_resp.message.content.trim().to_string();

        // Strip markdown code fences if present (models sometimes wrap in ```json...```)
        let json_str = strip_code_fences(&raw);

        let classification: Classification = serde_json::from_str(json_str).map_err(|e| {
            tracing::warn!(
                raw_response = %raw,
                error = %e,
                "failed to parse classification, falling back to remote_llm"
            );
            anyhow::anyhow!("classification parse error: {e}")
        })?;

        tracing::debug!(
            route = %classification.route,
            confidence = ?classification.confidence,
            reasoning = ?classification.reasoning,
            "router classification result"
        );

        Ok(classification)
    }

    async fn select_tools_with_local_model(
        &self,
        config: &RouterConfig,
        input: &ToolSelectionInput,
    ) -> anyhow::Result<Option<Vec<String>>> {
        if matches!(config.llm_mode(), LlmMode::RemoteOnly) {
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

        let request = OllamaRequest {
            model: config.model.clone(),
            messages: vec![
                OllamaMessage {
                    role: "system".to_string(),
                    content: TOOL_SELECTION_PROMPT.to_string(),
                },
                OllamaMessage {
                    role: "user".to_string(),
                    content: payload.to_string(),
                },
            ],
            stream: false,
            options: Some(OllamaOptions {
                temperature: config.temperature.min(0.2),
            }),
        };

        let url = format!("{}/api/chat", config.ollama_endpoint);
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ollama tool selection request failed: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("ollama tool selection returned {status}: {body}");
        }

        let ollama_resp: OllamaResponse = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("ollama tool selection response parse failed: {e}"))?;
        let raw = ollama_resp.message.content.trim().to_string();
        let selected = parse_tool_selection_response(&raw, &candidate_names, input.max_tools)
            .map_err(|e| {
                tracing::warn!(
                    raw_response = %raw,
                    error = %e,
                    "failed to parse local tool selection"
                );
                e
            })?;

        tracing::debug!(
            selected = ?selected,
            "local LLM tool candidate selection result"
        );
        Ok(Some(selected))
    }

    /// Convert a Classification into a RouteDecision, attaching the appropriate
    /// system prompt for the target LLM based on which tiers are enabled.
    pub fn to_decision(classification: Classification, mode: LlmMode) -> RouteDecision {
        match classification.route.as_str() {
            "skill" => {
                let skill_name = classification
                    .skill_name
                    .unwrap_or_else(|| "unknown".to_string());
                RouteDecision::Skill { skill_name }
            }
            "local_llm" => {
                let prompt = match mode {
                    LlmMode::LocalOnly => LOCAL_LLM_SYSTEM_PROMPT_SOLO,
                    _ => LOCAL_LLM_SYSTEM_PROMPT,
                };
                let escalation_prompt = match mode {
                    LlmMode::BothEnabled => Some(REMOTE_LLM_SYSTEM_PROMPT.to_string()),
                    _ => None,
                };
                RouteDecision::LocalLlm {
                    system_prompt: Some(prompt.to_string()),
                    escalation_prompt,
                }
            }
            // Default to remote for any unrecognized classification
            _ => {
                let prompt = match mode {
                    LlmMode::RemoteOnly => REMOTE_LLM_SYSTEM_PROMPT_SOLO,
                    _ => REMOTE_LLM_SYSTEM_PROMPT,
                };
                RouteDecision::RemoteLlm {
                    system_prompt: Some(prompt.to_string()),
                }
            }
        }
    }
}

#[async_trait]
impl ModuleLifecycle for MultiLayerRouter {
    fn name(&self) -> &'static str {
        "multi_layer"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let local_enabled = ctx.config.router.local_llm_enabled;
        let remote_enabled = ctx.config.router.remote_llm_enabled;

        if !local_enabled && !remote_enabled {
            anyhow::bail!(
                "invalid router config: both router_local_llm_enabled and \
                 router_remote_llm_enabled are false — at least one must be true"
            );
        }

        let timeout_sec = ctx.config.router.timeout_sec;
        self.client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_sec))
            .build()
            .unwrap_or_default();
        let cfg = RouterConfig {
            ollama_endpoint: ctx.config.llm.ollama_endpoint.clone(),
            model: ctx.config.router.model.clone(),
            temperature: ctx.config.router.temperature,
            timeout_sec,
            local_llm_enabled: local_enabled,
            remote_llm_enabled: remote_enabled,
        };
        tracing::info!(
            model = %cfg.model,
            endpoint = %cfg.ollama_endpoint,
            temperature = cfg.temperature,
            timeout_sec = cfg.timeout_sec,
            local_llm = local_enabled,
            remote_llm = remote_enabled,
            mode = ?cfg.llm_mode(),
            "MultiLayerRouter initialized"
        );
        *self.config.write().await = Some(cfg);
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        let config = self.config.read().await;
        if let Some(cfg) = config.as_ref() {
            match self
                .client
                .get(format!("{}/api/version", cfg.ollama_endpoint))
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => ModuleHealth::Healthy,
                Ok(r) => ModuleHealth::Degraded(format!("ollama returned {}", r.status())),
                Err(e) => ModuleHealth::Degraded(format!(
                    "ollama unreachable: {e}, routing will fall back to remote"
                )),
            }
        } else {
            ModuleHealth::Unhealthy("not initialized".to_string())
        }
    }
}

#[async_trait]
impl RouterStrategy for MultiLayerRouter {
    async fn route(&self, input: &RouterInput) -> anyhow::Result<RouteDecision> {
        let config = self.config.read().await;
        let cfg = config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("router not initialized"))?;

        let mode = cfg.llm_mode();

        match self.classify(cfg, input).await {
            Ok(classification) => Ok(Self::to_decision(classification, mode)),
            Err(e) => {
                // On classification failure, fall back to the best available LLM
                tracing::warn!(
                    error = %e,
                    "router classification failed, falling back"
                );
                let fallback = match mode {
                    LlmMode::LocalOnly => RouteDecision::LocalLlm {
                        system_prompt: Some(LOCAL_LLM_SYSTEM_PROMPT_SOLO.to_string()),
                        escalation_prompt: None,
                    },
                    LlmMode::RemoteOnly => RouteDecision::RemoteLlm {
                        system_prompt: Some(REMOTE_LLM_SYSTEM_PROMPT_SOLO.to_string()),
                    },
                    _ => RouteDecision::RemoteLlm {
                        system_prompt: Some(REMOTE_LLM_SYSTEM_PROMPT.to_string()),
                    },
                };
                Ok(fallback)
            }
        }
    }

    async fn select_tool_candidates(
        &self,
        input: &ToolSelectionInput,
    ) -> anyhow::Result<Option<Vec<String>>> {
        let config = self.config.read().await;
        let cfg = config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("router not initialized"))?;
        self.select_tools_with_local_model(cfg, input).await
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Strip optional markdown code fences from LLM output.
/// Handles ```json\n...\n```, ```\n...\n```, and bare JSON.
pub fn strip_code_fences(s: &str) -> &str {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        // Skip optional language tag on first line
        let rest = if let Some(nl) = rest.find('\n') {
            &rest[nl + 1..]
        } else {
            rest
        };
        // Strip trailing ```
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

    tracing::debug!(reasoning = ?selection.reasoning, "parsed local tool selection");
    Ok(selected)
}
