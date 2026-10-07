use crate::config::{
    CachePolicyResourceSection, CacheSection, ConversationSection, MemoryCaptureMode,
    MemorySection, ToolsSection,
};
use crate::settings::values::{
    bool_setting, f32_setting, string_setting, u32_setting, u64_setting, usize_setting,
};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct InteractionPolicies {
    pub conversation: ConversationSection,
    pub tools: ToolsSection,
    pub cache: CacheSection,
    pub cache_policy: CachePolicyResourceSection,
    pub memory: MemorySection,
}

pub type SharedInteractionPolicies = Arc<RwLock<InteractionPolicies>>;

fn parse_memory_capture_mode(value: &str) -> MemoryCaptureMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => MemoryCaptureMode::None,
        "explicit_tool_only" | "explicit-tool-only" => MemoryCaptureMode::ExplicitToolOnly,
        _ => MemoryCaptureMode::LlmSelected,
    }
}

impl InteractionPolicies {
    pub fn new(
        conversation: ConversationSection,
        tools: ToolsSection,
        cache: CacheSection,
        cache_policy: CachePolicyResourceSection,
        memory: MemorySection,
    ) -> Self {
        Self {
            conversation,
            tools,
            cache,
            cache_policy,
            memory,
        }
    }

    pub fn apply_settings(
        &mut self,
        updates: &std::collections::BTreeMap<String, Value>,
    ) -> Vec<String> {
        let mut applied = Vec::new();

        if let Some(value) =
            usize_setting(updates, "core.policies.conversation.max_context_messages")
        {
            self.conversation.max_context_messages = value;
            applied.push("core.policies.conversation.max_context_messages".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.conversation.streaming_enabled") {
            self.conversation.streaming_enabled = value;
            applied.push("core.policies.conversation.streaming_enabled".to_string());
        }

        if let Some(value) = bool_setting(updates, "core.policies.cache.enabled") {
            self.cache.enabled = value;
            applied.push("core.policies.cache.enabled".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.cache.exact_match_enabled") {
            self.cache.exact_match_enabled = value;
            applied.push("core.policies.cache.exact_match_enabled".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.cache.semantic_match_enabled") {
            self.cache.semantic_match_enabled = value;
            applied.push("core.policies.cache.semantic_match_enabled".to_string());
        }
        if let Some(value) = f32_setting(updates, "core.policies.cache.semantic_threshold") {
            self.cache.semantic_threshold = value.clamp(0.0, 1.0);
            applied.push("core.policies.cache.semantic_threshold".to_string());
        }
        if let Some(value) = u64_setting(updates, "core.policies.cache.default_ttl_seconds") {
            self.cache.default_ttl_seconds = value;
            applied.push("core.policies.cache.default_ttl_seconds".to_string());
        }
        if let Some(value) = u64_setting(updates, "core.policies.cache.skill_cache_ttl_seconds") {
            self.cache.skill_cache_ttl_seconds = value;
            applied.push("core.policies.cache.skill_cache_ttl_seconds".to_string());
        }

        if let Some(value) = bool_setting(updates, "core.policies.memory.enabled") {
            self.memory.enabled = value;
            applied.push("core.policies.memory.enabled".to_string());
        }
        if let Some(value) = string_setting(updates, "core.policies.memory.capture_mode") {
            self.memory.capture_mode = parse_memory_capture_mode(&value);
            applied.push("core.policies.memory.capture_mode".to_string());
        }
        if let Some(value) = u64_setting(updates, "core.policies.memory.capture_timeout_sec") {
            self.memory.capture_timeout_sec = value.max(1);
            applied.push("core.policies.memory.capture_timeout_sec".to_string());
        }
        if let Some(value) = f32_setting(updates, "core.policies.memory.capture_min_confidence") {
            self.memory.capture_min_confidence = value.clamp(0.0, 1.0);
            applied.push("core.policies.memory.capture_min_confidence".to_string());
        }
        if let Some(value) = f32_setting(updates, "core.policies.memory.capture_min_importance") {
            self.memory.capture_min_importance = value.clamp(0.0, 1.0);
            applied.push("core.policies.memory.capture_min_importance".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.memory.sensitive_filter_enabled")
        {
            self.memory.sensitive_filter_enabled = value;
            applied.push("core.policies.memory.sensitive_filter_enabled".to_string());
        }
        if let Some(value) = u32_setting(updates, "core.policies.memory.graph_depth") {
            self.memory.graph_depth = value.clamp(1, 4);
            applied.push("core.policies.memory.graph_depth".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.memory.consolidation_enabled") {
            self.memory.consolidation_enabled = value;
            applied.push("core.policies.memory.consolidation_enabled".to_string());
        }
        if let Some(value) = updates.get("core.policies.memory.paused_until") {
            self.memory.paused_until = value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            applied.push("core.policies.memory.paused_until".to_string());
        }
        if let Some(value) = updates
            .get("core.policies.memory.retrieval_layers")
            .and_then(|value| value.as_array())
        {
            self.memory.retrieval_layers = value
                .iter()
                .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                .collect();
            applied.push("core.policies.memory.retrieval_layers".to_string());
        }
        if let Some(value) =
            usize_setting(updates, "core.policies.memory.max_injected_memory_chars")
        {
            self.memory.max_injected_memory_chars = value.clamp(256, 12_000);
            applied.push("core.policies.memory.max_injected_memory_chars".to_string());
        }
        if let Some(value) = u32_setting(updates, "core.policies.memory.max_retrieval_results") {
            self.memory.max_retrieval_results = value;
            applied.push("core.policies.memory.max_retrieval_results".to_string());
        }
        if let Some(value) = f32_setting(updates, "core.policies.memory.similarity_threshold") {
            self.memory.similarity_threshold = value.clamp(0.0, 1.0);
            applied.push("core.policies.memory.similarity_threshold".to_string());
        }

        if let Some(value) = u32_setting(updates, "core.policies.tools.max_tool_call_depth") {
            self.tools.max_tool_call_depth = value;
            applied.push("core.policies.tools.max_tool_call_depth".to_string());
        }
        if let Some(value) = u64_setting(updates, "core.policies.tools.tool_call_timeout_sec") {
            self.tools.tool_call_timeout_sec = value;
            applied.push("core.policies.tools.tool_call_timeout_sec".to_string());
        }
        if let Some(value) = usize_setting(updates, "core.policies.tools.max_tools_per_turn") {
            self.tools.max_tools_per_turn = value;
            applied.push("core.policies.tools.max_tools_per_turn".to_string());
        }
        if let Some(value) = usize_setting(updates, "core.policies.tools.candidate_pool_size") {
            self.tools.candidate_pool_size = value;
            applied.push("core.policies.tools.candidate_pool_size".to_string());
        }
        if let Some(value) =
            bool_setting(updates, "core.policies.tools.llm_candidate_filter_enabled")
        {
            self.tools.llm_candidate_filter_enabled = value;
            applied.push("core.policies.tools.llm_candidate_filter_enabled".to_string());
        }
        if let Some(value) = usize_setting(
            updates,
            "core.policies.tools.llm_candidate_filter_threshold",
        ) {
            self.tools.llm_candidate_filter_threshold = value;
            applied.push("core.policies.tools.llm_candidate_filter_threshold".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.tools.allow_plugin_tools") {
            self.tools.allow_plugin_tools = value;
            applied.push("core.policies.tools.allow_plugin_tools".to_string());
        }
        if let Some(value) = bool_setting(updates, "core.policies.tools.indexed_search_enabled") {
            self.tools.indexed_search_enabled = value;
            applied.push("core.policies.tools.indexed_search_enabled".to_string());
        }
        if let Some(value) =
            f32_setting(updates, "core.policies.tools.indexed_search_min_similarity")
        {
            self.tools.indexed_search_min_similarity = value.clamp(0.0, 1.0);
            applied.push("core.policies.tools.indexed_search_min_similarity".to_string());
        }
        if let Some(value) = bool_setting(
            updates,
            "core.policies.tools.indexed_search_refresh_missing_embeddings",
        ) {
            self.tools.indexed_search_refresh_missing_embeddings = value;
            applied
                .push("core.policies.tools.indexed_search_refresh_missing_embeddings".to_string());
        }

        applied
    }
}
