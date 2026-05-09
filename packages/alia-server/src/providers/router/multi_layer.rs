//! MultiLayerRouter: Intent classification via local Ollama model.
//!
//! Uses a lightweight local model (e.g. Qwen3-4B) to classify user intent,
//! then routes to the appropriate execution path:
//!   - SKILL → matched skill with extracted parameters
//!   - LOCAL_LLM → simple chat handled by local Ollama model
//!   - REMOTE_LLM → complex reasoning forwarded to remote LLM (GPT-4o / Claude)

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::module::lifecycle::{ModuleContext, ModuleHealth, ModuleLifecycle};
use crate::providers::adapter::{RouteDecision, RouterInput, RouterStrategy};

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

// ── Classification result ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Classification {
    route: String,
    #[serde(default)]
    skill_name: Option<String>,
    #[serde(default)]
    params: Option<serde_json::Value>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    reasoning: Option<String>,
}

// ── Config ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct RouterConfig {
    ollama_endpoint: String,
    model: String,
    temperature: f32,
    timeout_sec: u64,
}

// ── System prompt ──────────────────────────────────────────────────────

const CLASSIFICATION_SYSTEM_PROMPT: &str = r#"You are a routing classifier for an AI assistant. Your job is to analyze the user's message and decide which execution path to use.

You MUST respond with ONLY a JSON object (no markdown fences, no explanation). The JSON schema:

{
  "route": "skill" | "local_llm" | "remote_llm",
  "skill_name": "<skill name if route=skill, else omit>",
  "params": { "<extracted parameters if route=skill, else omit>" },
  "confidence": <0.0 to 1.0>,
  "reasoning": "<one-sentence explanation>"
}

Routing rules:
- "skill": Deterministic commands like smart home control ("turn on lights", "set AC to 26°C"), timer/alarm setting, device queries. Extract the skill name and parameters.
- "local_llm": Simple chit-chat, greetings, confirmations, short factual answers, one-line replies that don't need deep reasoning.
- "remote_llm": Complex reasoning, multi-step analysis, creative writing, code generation, long context conversations, anything requiring deep understanding or personalization.

When in doubt, prefer "remote_llm" over "local_llm"."#;

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
        // Build messages: system prompt + conversation context (last few) + user message
        let mut messages = vec![OllamaMessage {
            role: "system".to_string(),
            content: CLASSIFICATION_SYSTEM_PROMPT.to_string(),
        }];

        // Include recent conversation context (last 4 turns max) for better classification
        let context_window = input.conversation_context.len().min(4);
        for msg in input.conversation_context.iter().rev().take(context_window).rev() {
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

    /// Convert a Classification into a RouteDecision
    fn to_decision(classification: Classification) -> RouteDecision {
        match classification.route.as_str() {
            "skill" => {
                let skill_name = classification
                    .skill_name
                    .unwrap_or_else(|| "unknown".to_string());
                let params = classification
                    .params
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                RouteDecision::Skill { skill_name, params }
            }
            "local_llm" => RouteDecision::LocalLlm,
            // Default to remote for any unrecognized classification
            _ => RouteDecision::RemoteLlm,
        }
    }
}

#[async_trait]
impl ModuleLifecycle for MultiLayerRouter {
    fn name(&self) -> &'static str {
        "multi_layer"
    }

    async fn init(&mut self, ctx: &ModuleContext) -> anyhow::Result<()> {
        let timeout_sec = ctx.config.ai.router_timeout_sec;
        self.client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_sec))
            .build()
            .unwrap_or_default();
        let cfg = RouterConfig {
            ollama_endpoint: ctx.config.ai.ollama_endpoint.clone(),
            model: ctx.config.ai.router_model.clone(),
            temperature: ctx.config.ai.router_temperature,
            timeout_sec,
        };
        tracing::info!(
            model = %cfg.model,
            endpoint = %cfg.ollama_endpoint,
            temperature = cfg.temperature,
            timeout_sec = cfg.timeout_sec,
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
                Ok(r) => ModuleHealth::Degraded(format!(
                    "ollama returned {}",
                    r.status()
                )),
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

        match self.classify(cfg, input).await {
            Ok(classification) => Ok(Self::to_decision(classification)),
            Err(e) => {
                // On classification failure, fall back to remote LLM (safe default)
                tracing::warn!(
                    error = %e,
                    "router classification failed, falling back to remote_llm"
                );
                Ok(RouteDecision::RemoteLlm)
            }
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Strip optional markdown code fences from LLM output.
/// Handles ```json\n...\n```, ```\n...\n```, and bare JSON.
fn strip_code_fences(s: &str) -> &str {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_code_fences_bare_json() {
        let input = r#"{"route": "remote_llm", "confidence": 0.9}"#;
        assert_eq!(strip_code_fences(input), input);
    }

    #[test]
    fn test_strip_code_fences_with_json_tag() {
        let input = "```json\n{\"route\": \"local_llm\"}\n```";
        assert_eq!(strip_code_fences(input), "{\"route\": \"local_llm\"}");
    }

    #[test]
    fn test_strip_code_fences_without_tag() {
        let input = "```\n{\"route\": \"skill\"}\n```";
        assert_eq!(strip_code_fences(input), "{\"route\": \"skill\"}");
    }

    #[test]
    fn test_to_decision_skill() {
        let c = Classification {
            route: "skill".to_string(),
            skill_name: Some("smart_home".to_string()),
            params: Some(serde_json::json!({"action": "turn_on", "device": "light"})),
            confidence: Some(0.95),
            reasoning: Some("deterministic command".to_string()),
        };
        match MultiLayerRouter::to_decision(c) {
            RouteDecision::Skill { skill_name, params } => {
                assert_eq!(skill_name, "smart_home");
                assert_eq!(params["action"], "turn_on");
            }
            other => panic!("expected Skill, got {other:?}"),
        }
    }

    #[test]
    fn test_to_decision_local_llm() {
        let c = Classification {
            route: "local_llm".to_string(),
            skill_name: None,
            params: None,
            confidence: Some(0.8),
            reasoning: None,
        };
        assert!(matches!(
            MultiLayerRouter::to_decision(c),
            RouteDecision::LocalLlm
        ));
    }

    #[test]
    fn test_to_decision_remote_llm() {
        let c = Classification {
            route: "remote_llm".to_string(),
            skill_name: None,
            params: None,
            confidence: Some(0.9),
            reasoning: None,
        };
        assert!(matches!(
            MultiLayerRouter::to_decision(c),
            RouteDecision::RemoteLlm
        ));
    }

    #[test]
    fn test_to_decision_unknown_falls_back() {
        let c = Classification {
            route: "unknown_thing".to_string(),
            skill_name: None,
            params: None,
            confidence: None,
            reasoning: None,
        };
        assert!(matches!(
            MultiLayerRouter::to_decision(c),
            RouteDecision::RemoteLlm
        ));
    }
}
