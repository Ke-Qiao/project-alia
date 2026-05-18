use std::collections::{HashMap, HashSet};
use std::time::Duration;

use uuid::Uuid;

use super::AgentTrace;
use super::debug::{
    debug_component_llm_call, debug_component_tool_calls, debug_component_tool_result,
};
use super::turn::{VoiceTurn, cancellable_stage, turn_is_interrupted};
use crate::config::ToolsSection;
use crate::module::ModuleRegistry;
use crate::providers::adapter::{
    ChatMessage, LlmAdapter, LlmResponse, LlmToolCall, ToolCall, ToolCandidateInfo, ToolContext,
    ToolDefinition, ToolResult, ToolSelectionInput,
};
use crate::registry::is_plugin_source;

const TEXT_TOOL_CALL_SCHEMA: &str = "alia.tool_calls.v1";
const MAX_TEXT_TOOL_CALLS: usize = 8;
const EMPTY_TOOL_RESPONSE_FALLBACK: &str = "抱歉，我刚才没有生成有效回复，请再试一次。";
const TOOL_TEXT_PROTOCOL_RETRY_PROMPT: &str = r#"The previous provider tool-call response was empty.
If a tool is needed, reply with exactly one JSON object using the `alia.tool_calls.v1` text tool-call protocol.
If no tool is needed, answer the user concisely in natural language.
Do not return an empty message."#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallProtocol {
    Native,
    TextFallback,
}

#[derive(Debug, Clone)]
pub(super) struct ToolLoopOutcome {
    pub text: String,
    pub used_tools: bool,
}

const TOOL_USE_SYSTEM_PROMPT: &str = r#"Tool use rules:
- Skills are cognitive context only. Never try to call a skill by name.
- Tools are the only executable units. When an appropriate tool is available, call the tool instead of describing imaginary results.
- Use native tool/function calling whenever the provider supports it. Tool arguments must be valid JSON matching the tool's parameter schema.
- If native tool calling is unavailable and a tool is needed, reply with exactly one JSON object and no prose:
  {"schema":"alia.tool_calls.v1","tool_calls":[{"name":"tool_name","arguments":{}}]}
- The JSON fallback is executable only when the entire assistant message is that JSON object, optionally wrapped in one `json` code fence.
- Never write pseudo function syntax such as `memory_store(...)` or `tool_name(...)` in user-facing text.
- If the user asks you to show, repeat, quote, explain, or debug prompts, tool formats, JSON examples, or available tools, do not use the JSON fallback; answer as normal user-facing text.
- Only call tools listed in Available tools for this turn. Calls to any other tool will be rejected.
- If no tool is needed, answer normally in natural language.
- Never put raw tool-call JSON, function names, or hidden arguments in the final user-facing answer.
- After tool results are provided, produce only a natural-language answer for the user.
- If a tool result says confirmation is required, ask the user to confirm before claiming the action was done.
"#;

pub(super) async fn run_llm_tool_loop(
    modules: &ModuleRegistry,
    llm: &dyn LlmAdapter,
    base_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    user_id: Uuid,
    session_id: Option<Uuid>,
    channel_id: &str,
    user_id_text: &str,
    route: &'static str,
    tools_config: &ToolsSection,
    debug_output: bool,
    trace: &mut AgentTrace,
    turn: Option<&VoiceTurn>,
) -> anyhow::Result<ToolLoopOutcome> {
    let mut messages = inject_tool_prompt(base_messages, &tools)?;
    let tool_map = tools
        .iter()
        .map(|tool| (tool.name.clone(), tool.clone()))
        .collect::<HashMap<_, _>>();
    let ctx = ToolContext {
        user_id,
        conversation_id: session_id,
        source: "ai".to_string(),
        channel_id: Some(channel_id.to_string()),
    };
    let max_depth = tools_config.max_tool_call_depth.max(1);
    let mut last_tool_display: Option<String> = None;
    let mut called_tool_names = Vec::new();

    for depth in 0..max_depth {
        debug_component_llm_call(
            debug_output,
            "llm",
            "tool_call",
            route,
            &messages,
            Some((channel_id, user_id_text)),
        );

        let mut response = match cancellable_stage(
            turn,
            "llm_tool_call",
            llm.chat_with_tools(&messages, &tools),
        )
        .await?
        {
            Some(response) => response,
            None => {
                return Ok(ToolLoopOutcome {
                    text: String::new(),
                    used_tools: !called_tool_names.is_empty(),
                });
            }
        };
        let mut protocol = normalize_tool_response(&mut response)?;
        if response.tool_calls.is_empty()
            && response.content.trim().is_empty()
            && !turn_is_interrupted(turn)
        {
            tracing::warn!(
                depth,
                channel_id,
                user_id = user_id_text,
                "LLM returned empty tool response; retrying with text tool protocol"
            );
            let mut retry_messages = messages.clone();
            retry_messages.push(ChatMessage::new("system", TOOL_TEXT_PROTOCOL_RETRY_PROMPT));
            debug_component_llm_call(
                debug_output,
                "llm",
                "tool_call_retry",
                route,
                &retry_messages,
                Some((channel_id, user_id_text)),
            );
            let retry_text = match cancellable_stage(
                turn,
                "llm_tool_call_text_retry",
                llm.chat(&retry_messages),
            )
            .await?
            {
                Some(retry_text) => retry_text,
                None => {
                    return Ok(ToolLoopOutcome {
                        text: String::new(),
                        used_tools: !called_tool_names.is_empty(),
                    });
                }
            };
            response = LlmResponse::text(retry_text);
            protocol = normalize_tool_response(&mut response)?;
        }

        if response.tool_calls.is_empty() {
            if response.content.trim().is_empty() {
                tracing::warn!(
                    depth,
                    channel_id,
                    user_id = user_id_text,
                    "LLM returned empty response after tool retry"
                );
                return Ok(ToolLoopOutcome {
                    text: non_empty_tool_fallback(last_tool_display.as_deref()),
                    used_tools: !called_tool_names.is_empty(),
                });
            }
            return Ok(ToolLoopOutcome {
                text: response.content,
                used_tools: !called_tool_names.is_empty(),
            });
        }

        let tool_calls = response
            .tool_calls
            .iter()
            .enumerate()
            .map(|(index, call)| normalize_tool_call(call, depth, index))
            .collect::<Vec<_>>();

        debug_component_tool_calls(
            debug_output,
            depth + 1,
            &tool_calls,
            Some((channel_id, user_id_text)),
        );
        trace
            .record(
                modules,
                "tool_calls",
                "llm",
                Some(format!("{} tool call(s)", tool_calls.len())),
                serde_json::json!({
                    "depth": depth + 1,
                    "calls": tool_calls
                        .iter()
                        .map(|call| serde_json::json!({
                            "tool": call.tool_name,
                            "call_id": call.id,
                            "arguments": call.arguments,
                        }))
                        .collect::<Vec<_>>(),
                }),
            )
            .await;

        messages.push(tool_call_request_message(
            protocol,
            &response.content,
            &tool_calls,
        ));

        for call in &tool_calls {
            if !called_tool_names.contains(&call.tool_name) {
                called_tool_names.push(call.tool_name.clone());
            }
            let result =
                execute_pipeline_tool_call(modules, &call, &tool_map, tools_config, &ctx).await;
            trace
                .record(
                    modules,
                    "tool_result",
                    "tool",
                    Some(call.tool_name.clone()),
                    serde_json::json!({
                        "tool": call.tool_name,
                        "call_id": result.call_id,
                        "success": result.success,
                        "display": result.display,
                        "output": result.output,
                    }),
                )
                .await;
            if turn_is_interrupted(turn) {
                tracing::info!(
                    channel_id,
                    user_id = user_id_text,
                    tool = %call.tool_name,
                    "discarding tool result from interrupted voice turn"
                );
                return Ok(ToolLoopOutcome {
                    text: String::new(),
                    used_tools: !called_tool_names.is_empty(),
                });
            }
            debug_component_tool_result(
                debug_output,
                &call.tool_name,
                &result,
                Some((channel_id, user_id_text)),
            );
            if let Some(display) = result
                .display
                .as_ref()
                .map(|display| display.trim())
                .filter(|display| !display.is_empty())
            {
                last_tool_display = Some(display.to_string());
            }
            messages.push(tool_result_message(protocol, &call.tool_name, result)?);
        }
    }

    tracing::warn!(
        depth = max_depth,
        "LLM tool loop reached max_tool_call_depth; asking model for final answer"
    );
    messages.push(ChatMessage::new(
        "system",
        "Tool-call depth limit reached. Stop calling tools and give the user a concise final answer based on the tool results already available.",
    ));

    let final_response = match cancellable_stage(turn, "llm", llm.chat(&messages)).await? {
        Some(final_response) => final_response,
        None => String::new(),
    };
    Ok(ToolLoopOutcome {
        text: final_response,
        used_tools: !called_tool_names.is_empty(),
    })
}

fn normalize_tool_response(response: &mut LlmResponse) -> anyhow::Result<ToolCallProtocol> {
    if response.tool_calls.is_empty() {
        response.tool_calls = parse_text_tool_calls(&response.content)?;
        if !response.tool_calls.is_empty() {
            response.content.clear();
        }
        Ok(ToolCallProtocol::TextFallback)
    } else {
        Ok(ToolCallProtocol::Native)
    }
}

pub fn non_empty_tool_fallback(last_tool_display: Option<&str>) -> String {
    last_tool_display
        .map(str::trim)
        .filter(|display| !display.is_empty())
        .unwrap_or(EMPTY_TOOL_RESPONSE_FALLBACK)
        .to_string()
}

pub(super) async fn select_tools_for_skill(
    modules: &ModuleRegistry,
    allowed_names: &[String],
    user_text: &str,
    tools_config: &ToolsSection,
    channel_id: &str,
) -> anyhow::Result<Vec<ToolDefinition>> {
    if allowed_names.is_empty() {
        return Ok(Vec::new());
    }

    select_tools(
        modules,
        Some(allowed_names),
        user_text,
        tools_config,
        channel_id,
    )
    .await
}

pub(super) async fn select_all_tools(
    modules: &ModuleRegistry,
    user_text: &str,
    tools_config: &ToolsSection,
    channel_id: &str,
) -> anyhow::Result<Vec<ToolDefinition>> {
    select_tools(modules, None, user_text, tools_config, channel_id).await
}

async fn select_tools(
    modules: &ModuleRegistry,
    allowed_names: Option<&[String]>,
    user_text: &str,
    tools_config: &ToolsSection,
    channel_id: &str,
) -> anyhow::Result<Vec<ToolDefinition>> {
    let all_tools = modules.tools.list_tools().await?;
    let skill_scoped = allowed_names.is_some();
    let allowed = allowed_names.map(|allowed_names| {
        allowed_names
            .iter()
            .map(|name| name.as_str())
            .collect::<HashSet<_>>()
    });

    let tools = all_tools
        .into_iter()
        .filter(|tool| {
            if !tools_config.allow_plugin_tools && is_plugin_source(&tool.source) {
                return false;
            }
            allowed
                .as_ref()
                .is_none_or(|allowed| allowed.contains(tool.name.as_str()))
        })
        .collect::<Vec<_>>();

    if let Some(allowed) = allowed {
        let found = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<HashSet<_>>();
        for name in allowed.difference(&found) {
            tracing::warn!(tool = %name, "skill requested unavailable tool");
        }
    }

    let total = tools.len();
    let selected = select_ranked_or_llm_filtered_tools(
        modules,
        tools,
        user_text,
        tools_config,
        skill_scoped,
        channel_id,
    )
    .await;
    if selected.len() < total {
        tracing::debug!(
            total,
            selected = selected.len(),
            skill_scoped,
            "tool router selected bounded tool set for turn"
        );
    }

    Ok(selected)
}

async fn select_ranked_or_llm_filtered_tools(
    modules: &ModuleRegistry,
    tools: Vec<ToolDefinition>,
    user_text: &str,
    tools_config: &ToolsSection,
    skill_scoped: bool,
    channel_id: &str,
) -> Vec<ToolDefinition> {
    let max_tools_per_turn = tools_config.max_tools_per_turn;
    if tools.is_empty() || max_tools_per_turn == 0 {
        return Vec::new();
    }

    let candidate_pool_size = tools_config
        .candidate_pool_size
        .max(max_tools_per_turn)
        .max(1);
    let ranked = indexed_rank_tool_candidates(
        modules,
        &tools,
        user_text,
        candidate_pool_size,
        skill_scoped,
        tools_config,
    )
    .await
    .unwrap_or_else(|| rank_tool_candidates(tools, user_text, candidate_pool_size, skill_scoped));

    if ranked.is_empty() {
        return Vec::new();
    }

    let should_use_llm_filter = tools_config.llm_candidate_filter_enabled
        && ranked.len() > max_tools_per_turn
        && ranked.len()
            >= tools_config
                .llm_candidate_filter_threshold
                .max(max_tools_per_turn + 1);

    if should_use_llm_filter {
        let candidate_infos = ranked
            .iter()
            .map(|candidate| tool_candidate_info(&candidate.tool))
            .collect::<Vec<_>>();
        let input = ToolSelectionInput {
            text: user_text.to_string(),
            channel_id: channel_id.to_string(),
            candidates: candidate_infos,
            max_tools: max_tools_per_turn,
        };

        match modules.router.select_tool_candidates(&input).await {
            Ok(Some(names)) => {
                let selected = select_tools_by_name_order(&ranked, &names, max_tools_per_turn);
                if !selected.is_empty() || names.is_empty() {
                    tracing::debug!(
                        candidates = ranked.len(),
                        selected = selected.len(),
                        "minor LLM selected tool candidates"
                    );
                    return selected;
                }
                tracing::debug!(
                    candidates = ranked.len(),
                    "minor LLM selected no valid tool names; using ranked fallback"
                );
            }
            Ok(None) => {
                tracing::debug!("router does not provide LLM tool candidate filtering");
            }
            Err(error) => {
                tracing::warn!(error = %error, "LLM tool candidate filtering failed; using ranked fallback");
            }
        }
    }

    ranked
        .into_iter()
        .take(max_tools_per_turn)
        .map(|candidate| candidate.tool)
        .collect()
}

pub fn rank_and_limit_tools(
    tools: Vec<ToolDefinition>,
    user_text: &str,
    max_tools_per_turn: usize,
    skill_scoped: bool,
    _channel_id: &str,
) -> Vec<ToolDefinition> {
    if tools.is_empty() || max_tools_per_turn == 0 {
        return Vec::new();
    }

    if skill_scoped && tools.len() <= max_tools_per_turn {
        return tools;
    }

    rank_tool_candidates(tools, user_text, max_tools_per_turn, skill_scoped)
        .into_iter()
        .map(|candidate| candidate.tool)
        .collect()
}

#[derive(Debug, Clone)]
struct RankedToolCandidate {
    score: i32,
    index: usize,
    semantic_similarity: Option<f32>,
    tool: ToolDefinition,
}

async fn indexed_rank_tool_candidates(
    modules: &ModuleRegistry,
    tools: &[ToolDefinition],
    user_text: &str,
    limit: usize,
    skill_scoped: bool,
    tools_config: &ToolsSection,
) -> Option<Vec<RankedToolCandidate>> {
    if !tools_config.indexed_search_enabled
        || tools.is_empty()
        || limit == 0
        || user_text.trim().is_empty()
        || modules.embedding.name() == "noop_embedding"
    {
        return None;
    }

    let query_embedding = match modules.embedding.embed(&[user_text.to_string()]).await {
        Ok(mut embeddings) => embeddings.pop().filter(|embedding| !embedding.is_empty())?,
        Err(error) => {
            tracing::debug!(error = ?error, "tool indexed search query embedding failed");
            return None;
        }
    };
    let embedding_model = modules.embedding.name().to_string();
    let tool_names = tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    let index_entries = match modules.tools.routing_index(&tool_names).await {
        Ok(entries) => entries,
        Err(error) => {
            tracing::debug!(error = ?error, "tool routing index lookup failed");
            Vec::new()
        }
    };
    let index_by_name = index_entries
        .into_iter()
        .map(|entry| (entry.name.clone(), entry))
        .collect::<HashMap<_, _>>();

    let mut embeddings_by_name = HashMap::<String, Vec<f32>>::new();
    let mut missing = Vec::<(String, String)>::new();
    for tool in tools {
        if let Some(entry) = index_by_name.get(&tool.name)
            && let Some(stored) = &entry.routing_embedding
            && stored.model.as_deref() == Some(embedding_model.as_str())
            && stored.vector.len() == query_embedding.len()
        {
            embeddings_by_name.insert(tool.name.clone(), stored.vector.clone());
            continue;
        }

        if tools_config.indexed_search_refresh_missing_embeddings {
            let routing_text = index_by_name
                .get(&tool.name)
                .map(|entry| entry.routing_text.clone())
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| tool_routing_text(tool));
            missing.push((tool.name.clone(), routing_text));
        }
    }

    if !missing.is_empty() {
        let texts = missing
            .iter()
            .map(|(_, text)| text.clone())
            .collect::<Vec<_>>();
        match modules.embedding.embed(&texts).await {
            Ok(vectors) => {
                for ((tool_name, _), vector) in missing.into_iter().zip(vectors.into_iter()) {
                    if vector.len() != query_embedding.len() || vector.is_empty() {
                        continue;
                    }
                    if let Err(error) = modules
                        .tools
                        .update_routing_embedding(&tool_name, &embedding_model, &vector)
                        .await
                    {
                        tracing::debug!(
                            tool = %tool_name,
                            error = ?error,
                            "failed to persist tool routing embedding"
                        );
                    }
                    embeddings_by_name.insert(tool_name, vector);
                }
            }
            Err(error) => {
                tracing::debug!(error = ?error, "tool routing embedding refresh failed");
            }
        }
    }

    if embeddings_by_name.is_empty() {
        return None;
    }

    let query = user_text.to_lowercase();
    let query_terms = tokenize_for_tool_routing(user_text);
    let min_similarity = tools_config.indexed_search_min_similarity.clamp(0.0, 1.0);
    let mut ranked = tools
        .iter()
        .cloned()
        .enumerate()
        .filter_map(|(index, tool)| {
            let lexical_score = score_tool_candidate(&tool, &query, &query_terms, skill_scoped);
            let semantic_similarity = embeddings_by_name
                .get(&tool.name)
                .and_then(|embedding| cosine_similarity(&query_embedding, embedding));
            let semantic_match = semantic_similarity.is_some_and(|score| score >= min_similarity);
            if !skill_scoped && lexical_score <= 0 && !semantic_match {
                return None;
            }

            let semantic_boost = semantic_similarity
                .map(|score| (score.clamp(0.0, 1.0) * 100.0).round() as i32)
                .unwrap_or_default();
            Some(RankedToolCandidate {
                score: lexical_score + semantic_boost,
                index,
                semantic_similarity,
                tool,
            })
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| {
                right
                    .semantic_similarity
                    .partial_cmp(&left.semantic_similarity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                left.tool
                    .require_confirmation
                    .cmp(&right.tool.require_confirmation)
            })
            .then_with(|| left.index.cmp(&right.index))
    });
    ranked.truncate(limit);

    tracing::debug!(
        candidates = tools.len(),
        selected = ranked.len(),
        "indexed tool candidate search applied"
    );
    Some(ranked)
}

fn rank_tool_candidates(
    tools: Vec<ToolDefinition>,
    user_text: &str,
    limit: usize,
    skill_scoped: bool,
) -> Vec<RankedToolCandidate> {
    if tools.is_empty() || limit == 0 {
        return Vec::new();
    }

    let query = user_text.to_lowercase();
    let query_terms = tokenize_for_tool_routing(user_text);
    let mut ranked = tools
        .into_iter()
        .enumerate()
        .map(|(index, tool)| {
            let score = score_tool_candidate(&tool, &query, &query_terms, skill_scoped);
            RankedToolCandidate {
                score,
                index,
                semantic_similarity: None,
                tool,
            }
        })
        .filter(|candidate| skill_scoped || candidate.score > 0)
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| {
                left.tool
                    .require_confirmation
                    .cmp(&right.tool.require_confirmation)
            })
            .then_with(|| left.index.cmp(&right.index))
    });

    ranked.into_iter().take(limit).collect()
}

fn select_tools_by_name_order(
    ranked: &[RankedToolCandidate],
    names: &[String],
    max_tools: usize,
) -> Vec<ToolDefinition> {
    let ranked_by_name = ranked
        .iter()
        .map(|candidate| (candidate.tool.name.as_str(), &candidate.tool))
        .collect::<HashMap<_, _>>();
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for name in names {
        if selected.len() >= max_tools {
            break;
        }
        if seen.insert(name.as_str())
            && let Some(tool) = ranked_by_name.get(name.as_str())
        {
            selected.push((*tool).clone());
        }
    }
    selected
}

fn tool_candidate_info(tool: &ToolDefinition) -> ToolCandidateInfo {
    ToolCandidateInfo {
        name: tool.name.clone(),
        description: tool.description.clone(),
        source: tool.source.clone(),
        require_confirmation: tool.require_confirmation,
        keywords: tool_candidate_keywords(tool),
    }
}

fn tool_candidate_keywords(tool: &ToolDefinition) -> Vec<String> {
    let mut keywords = tokenize_for_tool_routing(&format!(
        "{} {} {} {}",
        tool.name,
        tool.description,
        tool.source,
        schema_property_names(&tool.parameters).join(" ")
    ))
    .into_iter()
    .collect::<Vec<_>>();
    keywords.extend(
        tool_routing_hints(&tool.name.to_lowercase())
            .iter()
            .map(|hint| hint.to_string()),
    );
    keywords.extend(tool.routing_hints.iter().map(|hint| hint.to_lowercase()));
    keywords.sort();
    keywords.dedup();
    keywords.truncate(24);
    keywords
}

fn tool_routing_text(tool: &ToolDefinition) -> String {
    let mut hints = tool_routing_hints(&tool.name.to_lowercase())
        .iter()
        .map(|hint| (*hint).to_string())
        .collect::<Vec<_>>();
    hints.extend(tool.routing_hints.iter().cloned());
    format!(
        "{}\n{}\nsource: {}\nparameters: {}\nhints: {}",
        tool.name,
        tool.description,
        tool.source,
        schema_property_names(&tool.parameters).join(" "),
        hints.join(" ")
    )
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }

    let mut dot = 0.0_f32;
    let mut left_norm = 0.0_f32;
    let mut right_norm = 0.0_f32;
    for (left, right) in left.iter().zip(right.iter()) {
        if !left.is_finite() || !right.is_finite() {
            return None;
        }
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }

    if left_norm <= f32::EPSILON || right_norm <= f32::EPSILON {
        return None;
    }
    Some((dot / (left_norm.sqrt() * right_norm.sqrt())).clamp(-1.0, 1.0))
}

fn score_tool_candidate(
    tool: &ToolDefinition,
    query: &str,
    query_terms: &HashSet<String>,
    skill_scoped: bool,
) -> i32 {
    let mut score = if skill_scoped { 1 } else { 0 };
    let name = tool.name.to_lowercase();
    let searchable_text = format!(
        "{} {} {} {}",
        name.replace(['_', '-', '.'], " "),
        tool.description.to_lowercase(),
        tool.source.to_lowercase(),
        schema_property_names(&tool.parameters)
            .join(" ")
            .to_lowercase()
    );
    let searchable_terms = tokenize_for_tool_routing(&searchable_text);

    for term in query_terms {
        let term = term.as_str();
        if name == term {
            score += 16;
        } else if name.contains(term) {
            score += 8;
        }
        if searchable_terms.contains(term) {
            score += 3;
        }
    }

    for term in searchable_terms
        .iter()
        .filter(|term| term.chars().any(|ch| !ch.is_ascii()))
        .take(8)
    {
        if query.contains(term.as_str()) {
            score += 3;
        }
    }

    for hint in tool_routing_hints(&name) {
        if query.contains(hint) {
            score += 12;
        }
    }
    for hint in &tool.routing_hints {
        let hint = hint.trim().to_lowercase();
        if !hint.is_empty() && query.contains(&hint) {
            score += 12;
        }
    }

    if query.contains("http://") || query.contains("https://") {
        if name == "fetch" {
            score += 24;
        }
    }
    if looks_like_arithmetic(query) && name == "calculate" {
        score += 18;
    }

    score
}

fn tokenize_for_tool_routing(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter_map(|term| {
            let term = term.trim();
            (term.len() >= 3).then(|| term.to_string())
        })
        .collect()
}

fn schema_property_names(parameters: &serde_json::Value) -> Vec<String> {
    parameters
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default()
}

fn tool_routing_hints(tool_name: &str) -> &'static [&'static str] {
    match tool_name {
        "get_current_time" => &[
            "time", "date", "today", "tomorrow", "now", "clock", "timezone", "when", "时间",
            "几点", "今天", "明天", "现在", "日期",
        ],
        "fetch" => &[
            "fetch",
            "url",
            "http",
            "https",
            "website",
            "web",
            "page",
            "link",
            "download",
            "search",
            "lookup",
            "look up",
            "research",
            "latest",
            "current",
            "news",
            "网页",
            "链接",
            "网址",
            "访问",
            "抓取",
            "搜索",
            "查一下",
            "查询",
            "检索",
            "查找",
            "最新",
            "新闻",
            "资料",
            "当前",
        ],
        "calculate" => &[
            "calculate",
            "compute",
            "math",
            "sum",
            "plus",
            "minus",
            "multiply",
            "divide",
            "算",
            "计算",
            "加",
            "减",
            "乘",
            "除",
        ],
        "memory_search" => &[
            "remember",
            "memory",
            "recall",
            "preference",
            "past",
            "记得",
            "记忆",
            "偏好",
            "喜欢",
            "之前",
        ],
        "memory_store" => &[
            "remember this",
            "likes",
            "favorite",
            "preference",
            "save",
            "store",
            "note",
            "memorize",
            "记住",
            "保存",
            "记录",
            "喜欢",
            "最喜欢",
            "偏好",
        ],
        "schedule_task" => &[
            "schedule", "remind", "reminder", "timer", "alarm", "cron", "later", "提醒", "定时",
            "闹钟", "日程",
        ],
        "send_notification" => &[
            "notify",
            "notification",
            "alert",
            "message",
            "push",
            "通知",
            "提醒",
        ],
        "device_control" => &[
            "device",
            "light",
            "ac",
            "air conditioner",
            "fan",
            "turn on",
            "turn off",
            "brightness",
            "temperature",
            "volume",
            "speaker",
            "设备",
            "灯",
            "空调",
            "风扇",
            "打开",
            "关闭",
            "音量",
            "扬声器",
            "温度",
        ],
        "sensor_read" => &[
            "sensor",
            "temperature",
            "humidity",
            "value",
            "read",
            "status",
            "传感器",
            "温度",
            "湿度",
            "读数",
            "状态",
        ],
        "sensor_list" => &[
            "sensor",
            "sensors",
            "list sensors",
            "all sensors",
            "online",
            "offline",
            "传感器",
            "列表",
        ],
        _ => &[],
    }
}

fn looks_like_arithmetic(query: &str) -> bool {
    let has_operator = query
        .chars()
        .any(|ch| matches!(ch, '+' | '*' | '/' | '^' | '='))
        || query.contains(" - ");
    let digit_count = query.chars().filter(|ch| ch.is_ascii_digit()).count();
    has_operator && digit_count >= 2
}

fn inject_tool_prompt(
    mut messages: Vec<ChatMessage>,
    tools: &[ToolDefinition],
) -> anyhow::Result<Vec<ChatMessage>> {
    let tool_summary = tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
                "requires_confirmation": tool.require_confirmation,
            })
        })
        .collect::<Vec<_>>();
    let prompt = format!(
        "{TOOL_USE_SYSTEM_PROMPT}\nAvailable tools:\n{}",
        serde_json::to_string_pretty(&tool_summary)?
    );

    let insert_at = messages
        .iter()
        .rposition(|message| message.role == "system")
        .map(|index| index + 1)
        .unwrap_or(0);
    messages.insert(insert_at, ChatMessage::new("system", prompt));
    Ok(messages)
}

async fn execute_pipeline_tool_call(
    modules: &ModuleRegistry,
    call: &LlmToolCall,
    tool_map: &HashMap<String, ToolDefinition>,
    tools_config: &ToolsSection,
    ctx: &ToolContext,
) -> ToolResult {
    let Some(definition) = tool_map.get(&call.tool_name) else {
        return ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: serde_json::json!({
                "error": "tool_not_available",
                "tool": call.tool_name,
            }),
            display: Some(format!("Tool '{}' is not available", call.tool_name)),
        };
    };

    if tool_requires_confirmation(definition, tools_config) {
        return ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: serde_json::json!({
                "requires_confirmation": true,
                "tool": call.tool_name,
                "arguments": call.arguments.clone(),
                "message": "This tool requires user confirmation before execution.",
            }),
            display: Some(format!("Please confirm before I run '{}'.", call.tool_name)),
        };
    }

    let tool_call = ToolCall {
        tool_name: call.tool_name.clone(),
        arguments: call.arguments.clone(),
        call_id: call.id.clone(),
    };
    let timeout = Duration::from_secs(tools_config.tool_call_timeout_sec.max(1));

    match tokio::time::timeout(timeout, modules.tools.execute(tool_call, ctx)).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: serde_json::json!({
                "error": error.to_string(),
            }),
            display: Some(format!("Tool '{}' failed: {}", call.tool_name, error)),
        },
        Err(_) => ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: serde_json::json!({
                "error": "tool_timeout",
                "timeout_sec": tools_config.tool_call_timeout_sec,
            }),
            display: Some(format!("Tool '{}' timed out", call.tool_name)),
        },
    }
}

fn tool_requires_confirmation(tool: &ToolDefinition, tools_config: &ToolsSection) -> bool {
    tool.require_confirmation
        || tools_config
            .require_confirmation
            .iter()
            .any(|name| name == &tool.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                }
            }),
            routing_hints: Vec::new(),
            source: "builtin".to_string(),
            require_confirmation: false,
            required_role: None,
        }
    }

    #[test]
    fn cosine_similarity_scores_parallel_vectors_highest() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), Some(1.0));
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), Some(0.0));
        assert!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]).is_none());
    }

    #[test]
    fn tool_routing_text_contains_indexable_metadata() {
        let text = tool_routing_text(&tool("fetch", "Fetch a public HTTP URL"));

        assert!(text.contains("fetch"));
        assert!(text.contains("Fetch a public HTTP URL"));
        assert!(text.contains("parameters: query"));
        assert!(text.contains("latest"));
    }
}

pub fn normalize_tool_call(call: &LlmToolCall, depth: u32, index: usize) -> LlmToolCall {
    let id = if call.id.trim().is_empty() {
        format!("call_{depth}_{index}")
    } else {
        call.id.clone()
    };
    LlmToolCall {
        id,
        tool_name: call.tool_name.clone(),
        arguments: call.arguments.clone(),
    }
}

pub fn tool_call_request_message(
    protocol: ToolCallProtocol,
    content: &str,
    calls: &[LlmToolCall],
) -> ChatMessage {
    match protocol {
        ToolCallProtocol::Native => ChatMessage {
            role: "assistant".into(),
            content: content.to_string(),
            name: None,
            tool_calls: calls.to_vec(),
            ..ChatMessage::default()
        },
        ToolCallProtocol::TextFallback => {
            let names = calls
                .iter()
                .map(|call| call.tool_name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            ChatMessage::new(
                "assistant",
                format!("Tool call requested through text protocol: {names}"),
            )
        }
    }
}

pub fn tool_result_message(
    protocol: ToolCallProtocol,
    tool_name: &str,
    result: ToolResult,
) -> anyhow::Result<ChatMessage> {
    let content = serde_json::to_string(&serde_json::json!({
        "tool": tool_name,
        "success": result.success,
        "output": result.output,
        "display": result.display,
    }))?;

    match protocol {
        ToolCallProtocol::Native => Ok(ChatMessage {
            role: "tool".into(),
            content,
            name: Some(tool_name.to_string()),
            tool_call_id: Some(result.call_id),
            ..ChatMessage::default()
        }),
        ToolCallProtocol::TextFallback => Ok(ChatMessage::new(
            "system",
            format!(
                "Tool result for text-protocol call `{}` (call_id `{}`): {}\nUse this result to answer the user. If confirmation is required, ask the user to confirm before claiming the action was done.",
                tool_name, result.call_id, content
            ),
        )),
    }
}

pub fn parse_text_tool_calls(content: &str) -> anyhow::Result<Vec<LlmToolCall>> {
    let Some(candidate) = standalone_tool_call_json(content) else {
        return Ok(parse_pseudo_function_tool_call(content)
            .into_iter()
            .collect());
    };
    if !candidate.contains("\"tool_calls\"") {
        return Ok(Vec::new());
    }

    let value: serde_json::Value = match serde_json::from_str(candidate) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "ignoring malformed standalone text tool-call envelope"
            );
            return Ok(Vec::new());
        }
    };

    let Some(object) = value.as_object() else {
        return Ok(Vec::new());
    };
    if object.get("schema").and_then(|value| value.as_str()) != Some(TEXT_TOOL_CALL_SCHEMA) {
        tracing::warn!(
            schema = ?object.get("schema"),
            expected = TEXT_TOOL_CALL_SCHEMA,
            "ignoring text tool-call envelope with missing or invalid schema"
        );
        return Ok(Vec::new());
    };
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "schema" | "tool_calls"))
    {
        tracing::warn!("ignoring text tool-call envelope with unexpected top-level fields");
        return Ok(Vec::new());
    }

    let Some(calls) = object.get("tool_calls").and_then(|value| value.as_array()) else {
        tracing::warn!("ignoring text tool-call envelope without a tool_calls array");
        return Ok(Vec::new());
    };
    let mut parsed = Vec::new();
    for (index, call) in calls.iter().take(MAX_TEXT_TOOL_CALLS).enumerate() {
        let Some(call_object) = call.as_object() else {
            tracing::warn!(index, "ignoring non-object text tool call");
            continue;
        };
        if call_object
            .keys()
            .any(|key| !matches!(key.as_str(), "id" | "name" | "tool_name" | "arguments"))
        {
            tracing::warn!(index, "ignoring text tool call with unexpected fields");
            continue;
        }
        let Some(name) = call
            .get("name")
            .or_else(|| call.get("tool_name"))
            .and_then(|value| value.as_str())
            .filter(|name| is_valid_text_tool_name(name))
        else {
            tracing::warn!(index, "ignoring text tool call without a valid name");
            continue;
        };
        let arguments = call
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let arguments = match arguments {
            serde_json::Value::String(raw) => match serde_json::from_str::<serde_json::Value>(&raw)
            {
                Ok(value) if value.is_object() => value,
                Ok(_) => {
                    tracing::warn!(
                        index,
                        tool = %name,
                        "ignoring text tool call with non-object JSON arguments"
                    );
                    continue;
                }
                Err(error) => {
                    tracing::warn!(
                        index,
                        tool = %name,
                        error = %error,
                        "ignoring text tool call with invalid JSON arguments"
                    );
                    continue;
                }
            },
            value if value.is_object() => value,
            _ => {
                tracing::warn!(
                    index,
                    tool = %name,
                    "ignoring text tool call with non-object arguments"
                );
                continue;
            }
        };
        let id = call
            .get("id")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_default();
        parsed.push(LlmToolCall {
            id,
            tool_name: name.to_string(),
            arguments,
        });
    }
    if calls.len() > MAX_TEXT_TOOL_CALLS {
        tracing::warn!(
            count = calls.len(),
            limit = MAX_TEXT_TOOL_CALLS,
            "truncated oversized text tool-call envelope"
        );
    }

    Ok(parsed)
}

fn parse_pseudo_function_tool_call(content: &str) -> Option<LlmToolCall> {
    let trimmed = content.trim_start();
    let name_end = trimmed
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        .last()
        .map(|(index, ch)| index + ch.len_utf8())?;
    let name = &trimmed[..name_end];
    if !is_valid_text_tool_name(name) {
        return None;
    }

    let rest = trimmed[name_end..].trim_start();
    if !rest.starts_with('(') {
        return None;
    }

    let closing = find_matching_paren(rest)?;
    let args = &rest[1..closing];
    let arguments = parse_pseudo_function_arguments(args).ok()?;
    Some(LlmToolCall {
        id: String::new(),
        tool_name: name.to_string(),
        arguments,
    })
}

fn find_matching_paren(input: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;

    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = true;
                quote = ch;
            }
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }

    None
}

fn parse_pseudo_function_arguments(input: &str) -> anyhow::Result<serde_json::Value> {
    let mut object = serde_json::Map::new();
    for item in split_pseudo_args(input) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let Some((key, value)) = split_pseudo_key_value(item) else {
            anyhow::bail!("invalid pseudo tool argument '{item}'");
        };
        let key = key.trim();
        if !is_valid_text_tool_name(key) {
            anyhow::bail!("invalid pseudo tool argument key '{key}'");
        }
        object.insert(key.to_string(), parse_pseudo_value(value.trim()));
    }

    Ok(serde_json::Value::Object(object))
}

fn split_pseudo_args(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;
    let mut bracket_depth = 0usize;

    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = true;
                quote = ch;
            }
            '[' | '{' => bracket_depth += 1,
            ']' | '}' => bracket_depth = bracket_depth.saturating_sub(1),
            ',' if bracket_depth == 0 => {
                parts.push(&input[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&input[start..]);
    parts
}

fn split_pseudo_key_value(input: &str) -> Option<(&str, &str)> {
    let mut in_string = false;
    let mut quote = '\0';
    let mut escaped = false;

    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = true;
                quote = ch;
            }
            '=' | ':' => return Some((&input[..index], &input[index + ch.len_utf8()..])),
            _ => {}
        }
    }

    None
}

fn parse_pseudo_value(input: &str) -> serde_json::Value {
    let trimmed = input.trim();
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        return parse_quoted_pseudo_string(trimmed)
            .map(serde_json::Value::String)
            .unwrap_or_else(|| serde_json::Value::String(trimmed.to_string()));
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return value;
    }
    if let Ok(number) = trimmed.parse::<f64>() {
        return serde_json::json!(number);
    }
    serde_json::Value::String(trimmed.to_string())
}

fn parse_quoted_pseudo_string(input: &str) -> Option<String> {
    let quote = input.chars().next()?;
    let mut value = String::new();
    let mut escaped = false;

    for ch in input[quote.len_utf8()..].chars() {
        if escaped {
            let decoded = match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '"' => '"',
                '\'' => '\'',
                other => other,
            };
            value.push(decoded);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return Some(value);
        } else {
            value.push(ch);
        }
    }

    None
}

fn standalone_tool_call_json(content: &str) -> Option<&str> {
    let trimmed = content.trim();

    if trimmed.starts_with("```") {
        return standalone_json_fence(trimmed);
    }

    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        Some(trimmed)
    } else {
        None
    }
}

fn standalone_json_fence(trimmed: &str) -> Option<&str> {
    let body = trimmed.strip_prefix("```")?;
    let header_end = body.find('\n')?;
    let header = body[..header_end].trim();
    if !header.is_empty() && !header.eq_ignore_ascii_case("json") {
        return None;
    }

    let after_header = &body[header_end + 1..];
    let closing_start = after_header.rfind("```")?;
    if !after_header[closing_start + 3..].trim().is_empty() {
        return None;
    }
    let inner = after_header[..closing_start].trim();
    (inner.starts_with('{') && inner.ends_with('}')).then_some(inner)
}

fn is_valid_text_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    name.len() <= 128 && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}
