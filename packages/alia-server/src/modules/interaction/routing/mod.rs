//! Model routing and tool candidate selection within Interaction.
use crate::providers::adapter::ChatMessage;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RouterInput {
    pub text: String,
    pub user_id: Uuid,
    pub conversation_context: Vec<ChatMessage>,
    pub assistant_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCandidateInfo {
    pub name: String,
    pub description: String,
    pub source: String,
    pub require_confirmation: bool,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ToolSelectionInput {
    pub text: String,
    pub channel_id: String,
    pub assistant_name: String,
    pub candidates: Vec<ToolCandidateInfo>,
    pub max_tools: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteDecision {
    Skill {
        skill_name: String,
    },
    MinorLlm {
        system_prompt: Option<String>,
        escalation_prompt: Option<String>,
    },
    MajorLlm {
        system_prompt: Option<String>,
    },
}

mod service;

pub use service::{
    Classification, LlmMode, RouterConfig, RouterService, parse_tool_selection_response,
    strip_code_fences,
};
