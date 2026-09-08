use std::collections::{HashMap, HashSet};

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
    ToolDefinition, ToolResult, ToolRoutingEmbedding, ToolRoutingIndexEntry, ToolSelectionInput,
};
use crate::registry::is_plugin_source;

const TEXT_TOOL_CALL_SCHEMA: &str = "alia.tool_calls.v1";
const MAX_TEXT_TOOL_CALLS: usize = 8;
const EMPTY_TOOL_RESPONSE_FALLBACK: &str = "抱歉，我刚才没有生成有效回复，请再试一次。";
const PREFERRED_TOOL_SCORE_BOOST: i32 = 4;
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
        source_received_at: turn
            .map(|turn| turn.received_at)
            .unwrap_or_else(chrono::Utc::now),
        source_event_id: turn.and_then(|turn| turn.source_event_id.get()).copied(),
        request_id: trace.turn_id,
        invocation_key: String::new(),
        conversation_id: session_id,
        source: "ai".to_string(),
        channel_id: Some(channel_id.to_string()),
        cancel: turn.map(|turn| turn.token.clone()),
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

        let capabilities = llm.llm_capabilities().await;
        let tool_call_future = async {
            if capabilities.supports_streaming_tool_calls {
                llm.chat_stream_with_tools(&messages, &tools).await
            } else {
                llm.chat_with_tools(&messages, &tools).await
            }
        };
        let stage = if capabilities.supports_streaming_tool_calls {
            "llm_tool_call_stream"
        } else {
            "llm_tool_call"
        };
        let mut response = match cancellable_stage(turn, stage, tool_call_future).await? {
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

        for (index, call) in tool_calls.iter().enumerate() {
            if turn_is_interrupted(turn) {
                return Ok(ToolLoopOutcome {
                    text: String::new(),
                    used_tools: !called_tool_names.is_empty(),
                });
            }
            // Do not drop this execution on response cancellation. Its result
            // must be recorded even if the response no longer has a consumer.
            if !called_tool_names.contains(&call.tool_name) {
                called_tool_names.push(call.tool_name.clone());
            }
            let mut call_context = ctx.clone();
            call_context.invocation_key = format!("{depth}:{index}");
            let result = execute_pipeline_tool_call(modules, &call, &tool_map, &call_context).await;
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

#[derive(Debug, Clone, Default)]
pub(super) struct ToolSelection {
    pub available_tools: Vec<ToolDefinition>,
    pub selected_tools: Vec<ToolDefinition>,
    pub preferred_tool_names: Vec<String>,
}

pub(super) async fn select_tools_for_skill(
    modules: &ModuleRegistry,
    preferred_names: &[String],
    user_text: &str,
    tools_config: &ToolsSection,
    channel_id: &str,
    assistant_name: &str,
) -> anyhow::Result<ToolSelection> {
    select_tools(
        modules,
        Some(preferred_names),
        user_text,
        tools_config,
        channel_id,
        assistant_name,
    )
    .await
}

pub(super) async fn select_all_tools(
    modules: &ModuleRegistry,
    user_text: &str,
    tools_config: &ToolsSection,
    channel_id: &str,
    assistant_name: &str,
) -> anyhow::Result<ToolSelection> {
    select_tools(
        modules,
        None,
        user_text,
        tools_config,
        channel_id,
        assistant_name,
    )
    .await
}

async fn select_tools(
    modules: &ModuleRegistry,
    preferred_names: Option<&[String]>,
    user_text: &str,
    tools_config: &ToolsSection,
    channel_id: &str,
    assistant_name: &str,
) -> anyhow::Result<ToolSelection> {
    let all_tools = modules.tools.list_tools().await?;
    let skill_context = preferred_names.is_some();
    let preferred_names = preferred_names
        .map(preferred_tool_names)
        .unwrap_or_default();
    let mut preferred_tool_names = preferred_names.iter().cloned().collect::<Vec<_>>();
    preferred_tool_names.sort();

    let available_tools = all_tools
        .into_iter()
        .filter(|tool| tools_config.allow_plugin_tools || !is_plugin_source(&tool.source))
        .collect::<Vec<_>>();

    if !preferred_names.is_empty() {
        let found = available_tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<HashSet<_>>();
        for name in preferred_names
            .iter()
            .filter(|name| !found.contains(name.as_str()))
        {
            tracing::warn!(tool = %name, "skill preferred unavailable tool");
        }
    }

    let total = available_tools.len();
    let selected_tools = select_ranked_or_llm_filtered_tools(
        modules,
        available_tools.clone(),
        user_text,
        tools_config,
        &preferred_names,
        channel_id,
        assistant_name,
    )
    .await;
    if selected_tools.len() < total {
        tracing::debug!(
            total,
            selected = selected_tools.len(),
            skill_context,
            "tool router selected bounded tool set for turn"
        );
    }

    Ok(ToolSelection {
        available_tools,
        selected_tools,
        preferred_tool_names,
    })
}

fn preferred_tool_names(names: &[String]) -> HashSet<String> {
    names
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn preferred_tool_names_from_str(names: &[&str]) -> HashSet<String> {
    names
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub trait ToolPreferenceArg {
    fn to_preferred_tool_names(&self, tools: &[ToolDefinition]) -> HashSet<String>;
}

impl ToolPreferenceArg for bool {
    fn to_preferred_tool_names(&self, tools: &[ToolDefinition]) -> HashSet<String> {
        if *self {
            tools.iter().map(|tool| tool.name.clone()).collect()
        } else {
            HashSet::new()
        }
    }
}

impl ToolPreferenceArg for &[&str] {
    fn to_preferred_tool_names(&self, _tools: &[ToolDefinition]) -> HashSet<String> {
        preferred_tool_names_from_str(self)
    }
}

impl<const N: usize> ToolPreferenceArg for &[&str; N] {
    fn to_preferred_tool_names(&self, _tools: &[ToolDefinition]) -> HashSet<String> {
        preferred_tool_names_from_str(self.as_slice())
    }
}

async fn select_ranked_or_llm_filtered_tools(
    modules: &ModuleRegistry,
    tools: Vec<ToolDefinition>,
    user_text: &str,
    tools_config: &ToolsSection,
    preferred_names: &HashSet<String>,
    channel_id: &str,
    assistant_name: &str,
) -> Vec<ToolDefinition> {
    let max_tools_per_turn = tools_config.max_tools_per_turn;
    if tools.is_empty() || max_tools_per_turn == 0 {
        return Vec::new();
    }

    let candidate_pool_size = tools_config
        .candidate_pool_size
        .max(max_tools_per_turn)
        .max(1);
    let index_by_name = load_tool_routing_index(modules, &tools).await;
    let ranked = indexed_rank_tool_candidates(
        modules,
        &tools,
        user_text,
        candidate_pool_size,
        preferred_names,
        tools_config,
        &index_by_name,
    )
    .await
    .unwrap_or_else(|| {
        rank_tool_candidates(
            tools,
            user_text,
            candidate_pool_size,
            preferred_names,
            &index_by_name,
        )
    });

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
        let candidate_infos = ranked.iter().map(tool_candidate_info).collect::<Vec<_>>();
        let input = ToolSelectionInput {
            text: user_text.to_string(),
            channel_id: channel_id.to_string(),
            assistant_name: assistant_name.to_string(),
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
    preferred_names: impl ToolPreferenceArg,
    _channel_id: &str,
) -> Vec<ToolDefinition> {
    if tools.is_empty() || max_tools_per_turn == 0 {
        return Vec::new();
    }

    let preferred_names = preferred_names.to_preferred_tool_names(&tools);
    rank_tool_candidates(
        tools,
        user_text,
        max_tools_per_turn,
        &preferred_names,
        &HashMap::new(),
    )
    .into_iter()
    .map(|candidate| candidate.tool)
    .collect()
}

pub fn rank_and_limit_tools_with_index(
    tools: Vec<ToolDefinition>,
    index_entries: Vec<ToolRoutingIndexEntry>,
    user_text: &str,
    max_tools_per_turn: usize,
    preferred_names: impl ToolPreferenceArg,
    _channel_id: &str,
) -> Vec<ToolDefinition> {
    if tools.is_empty() || max_tools_per_turn == 0 {
        return Vec::new();
    }

    let index_by_name = index_entries
        .into_iter()
        .map(|entry| (entry.name.clone(), entry))
        .collect::<HashMap<_, _>>();
    let preferred_names = preferred_names.to_preferred_tool_names(&tools);
    rank_tool_candidates(
        tools,
        user_text,
        max_tools_per_turn,
        &preferred_names,
        &index_by_name,
    )
    .into_iter()
    .map(|candidate| candidate.tool)
    .collect()
}

#[derive(Debug, Clone)]
struct RankedToolCandidate {
    score: i32,
    index: usize,
    semantic_similarity: Option<f32>,
    routing: ToolRoutingMetadata,
    tool: ToolDefinition,
}

#[derive(Debug, Clone, Default)]
struct ToolRoutingMetadata {
    routing_text: String,
    routing_keywords: Vec<String>,
    routing_embedding: Option<ToolRoutingEmbedding>,
}

async fn load_tool_routing_index(
    modules: &ModuleRegistry,
    tools: &[ToolDefinition],
) -> HashMap<String, ToolRoutingIndexEntry> {
    if tools.is_empty() {
        return HashMap::new();
    }

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

    let missing = tools
        .iter()
        .filter(|tool| !index_by_name.contains_key(&tool.name))
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        tracing::debug!(
            tools = ?missing,
            "tool routing index missing entries; using ToolDefinition fallback metadata"
        );
    }

    index_by_name
}

async fn indexed_rank_tool_candidates(
    modules: &ModuleRegistry,
    tools: &[ToolDefinition],
    user_text: &str,
    limit: usize,
    preferred_names: &HashSet<String>,
    tools_config: &ToolsSection,
    index_by_name: &HashMap<String, ToolRoutingIndexEntry>,
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
    let mut embeddings_by_name = HashMap::<String, Vec<f32>>::new();
    let mut missing = Vec::<(String, String)>::new();
    for tool in tools {
        let routing = tool_routing_metadata(tool, index_by_name);
        if let Some(stored) = &routing.routing_embedding
            && stored.model.as_deref() == Some(embedding_model.as_str())
            && stored.vector.len() == query_embedding.len()
        {
            embeddings_by_name.insert(tool.name.clone(), stored.vector.clone());
            continue;
        }

        if tools_config.indexed_search_refresh_missing_embeddings {
            missing.push((tool.name.clone(), routing.routing_text));
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
            let routing = tool_routing_metadata(&tool, index_by_name);
            let lexical_score = score_tool_candidate(
                &tool,
                &routing,
                &query,
                &query_terms,
                preferred_names.contains(tool.name.as_str()),
            );
            let semantic_similarity = embeddings_by_name
                .get(&tool.name)
                .and_then(|embedding| cosine_similarity(&query_embedding, embedding));
            let semantic_match = semantic_similarity.is_some_and(|score| score >= min_similarity);
            if lexical_score <= 0 && !semantic_match {
                return None;
            }

            let semantic_boost = semantic_similarity
                .map(|score| (score.clamp(0.0, 1.0) * 100.0).round() as i32)
                .unwrap_or_default();
            Some(RankedToolCandidate {
                score: lexical_score + semantic_boost,
                index,
                semantic_similarity,
                routing,
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
    preferred_names: &HashSet<String>,
    index_by_name: &HashMap<String, ToolRoutingIndexEntry>,
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
            let routing = tool_routing_metadata(&tool, index_by_name);
            let score = score_tool_candidate(
                &tool,
                &routing,
                &query,
                &query_terms,
                preferred_names.contains(tool.name.as_str()),
            );
            RankedToolCandidate {
                score,
                index,
                semantic_similarity: None,
                routing,
                tool,
            }
        })
        .filter(|candidate| candidate.score > 0)
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

fn tool_candidate_info(candidate: &RankedToolCandidate) -> ToolCandidateInfo {
    ToolCandidateInfo {
        name: candidate.tool.name.clone(),
        description: candidate.tool.description.clone(),
        source: candidate.tool.source.clone(),
        require_confirmation: candidate.tool.require_confirmation,
        keywords: candidate.routing.routing_keywords.clone(),
    }
}

fn tool_routing_metadata(
    tool: &ToolDefinition,
    index_by_name: &HashMap<String, ToolRoutingIndexEntry>,
) -> ToolRoutingMetadata {
    if let Some(entry) = index_by_name.get(&tool.name) {
        let routing_text = entry
            .routing_text
            .trim()
            .is_empty()
            .then(|| fallback_tool_routing_text(tool))
            .unwrap_or_else(|| entry.routing_text.clone());
        let routing_keywords = if entry.routing_keywords.is_empty() {
            fallback_tool_routing_keywords(&routing_text)
        } else {
            normalized_keywords(entry.routing_keywords.clone())
        };
        return ToolRoutingMetadata {
            routing_text,
            routing_keywords,
            routing_embedding: entry.routing_embedding.clone(),
        };
    }

    let routing_text = fallback_tool_routing_text(tool);
    let routing_keywords = fallback_tool_routing_keywords(&routing_text);
    ToolRoutingMetadata {
        routing_text,
        routing_keywords,
        routing_embedding: None,
    }
}

fn fallback_tool_routing_keywords(routing_text: &str) -> Vec<String> {
    let mut keywords = tokenize_for_tool_routing(routing_text)
        .into_iter()
        .collect::<Vec<_>>();
    keywords.sort();
    keywords.dedup();
    keywords.truncate(24);
    keywords
}

fn normalized_keywords(keywords: Vec<String>) -> Vec<String> {
    let mut keywords = keywords
        .into_iter()
        .map(|keyword| keyword.trim().to_lowercase())
        .filter(|keyword| !keyword.is_empty())
        .collect::<Vec<_>>();
    keywords.sort();
    keywords.dedup();
    keywords.truncate(24);
    keywords
}

fn fallback_tool_routing_text(tool: &ToolDefinition) -> String {
    format!(
        "{}\n{}\nsource: {}\nparameters: {}\nhints: {}",
        tool.name,
        tool.description,
        tool.source,
        schema_property_names(&tool.parameters).join(" "),
        tool.routing_hints.join(" ")
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
    routing: &ToolRoutingMetadata,
    query: &str,
    query_terms: &HashSet<String>,
    preferred: bool,
) -> i32 {
    let mut score = if preferred {
        PREFERRED_TOOL_SCORE_BOOST
    } else {
        0
    };
    let name = tool.name.to_lowercase();
    let searchable_terms = tokenize_for_tool_routing(&routing.routing_text);
    let routing_keywords = routing
        .routing_keywords
        .iter()
        .map(|keyword| keyword.to_lowercase())
        .collect::<Vec<_>>();

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
        if routing_keywords.iter().any(|keyword| keyword == term) {
            score += 10;
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

    for keyword in &routing_keywords {
        if !keyword.is_empty() && query.contains(keyword) {
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
    ctx: &ToolContext,
) -> ToolResult {
    let Some(_) = tool_map.get(&call.tool_name) else {
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

    let tool_call = ToolCall {
        tool_name: call.tool_name.clone(),
        arguments: call.arguments.clone(),
        call_id: call.id.clone(),
    };
    match modules.tools.execute(tool_call, ctx).await {
        Ok(result) => result,
        Err(error) => ToolResult {
            call_id: call.id.clone(),
            success: false,
            output: serde_json::json!({
                "error": error.to_string(),
            }),
            display: Some(format!(
                "Execution receipt unavailable for '{}'; inspect the ledger before retrying: {}",
                call.tool_name, error
            )),
        },
    }
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
        let mut tool = tool("fetch", "Fetch a public HTTP URL");
        tool.routing_hints = vec!["latest".to_string()];
        let text = fallback_tool_routing_text(&tool);

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
