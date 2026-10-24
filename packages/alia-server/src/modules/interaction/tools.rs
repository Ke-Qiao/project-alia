use crate::providers::llm::LlmService;
use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use super::InteractionServices;
use super::debug::{debug_component_tool_calls, debug_component_tool_result};
use super::trace::AgentTrace;
use super::turn::{VoiceTurn, cancellable_stage, turn_is_interrupted};
use crate::config::ToolsSection;
use crate::registry::is_plugin_source;
use crate::{
    modules::interaction::routing::{ToolCandidateInfo, ToolSelectionInput},
    providers::adapter::{
        ChatMessage, ToolCall, ToolContext, ToolDefinition, ToolResult, ToolRoutingEmbedding,
        ToolRoutingIndexEntry,
    },
};

const PREFERRED_TOOL_SCORE_BOOST: i32 = 4;
const TOOL_USE_SYSTEM_PROMPT: &str = "Use only the native tools advertised for this turn. Tools are the only executable units; skills are context. Arguments must match the tool schema. Never claim an action succeeded without its execution receipt. Pending, unknown, or confirmation-required results do not authorize retry or claims of completion. After tool results, answer in natural language.";

#[derive(Debug, Clone)]
pub(super) struct ToolLoopOutcome {
    pub text: String,
    pub used_tools: bool,
}

/// Rig owns the native transcript and round state. Execution alone owns dispatch.
pub(super) async fn run_llm_tool_loop(
    modules: &InteractionServices,
    llm: &LlmService,
    base_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    user_id: Uuid,
    session_id: Option<Uuid>,
    channel_id: &str,
    user_id_text: &str,
    _route: &'static str,
    tools_config: &ToolsSection,
    debug_output: bool,
    trace: &mut AgentTrace,
    turn: Option<&VoiceTurn>,
) -> anyhow::Result<ToolLoopOutcome> {
    use futures::StreamExt;
    use rig_agent::run::{AgentRun, AgentRunStep, ModelTurn, RunSpec, prepare_request};
    use rig_core::{
        completion::CompletionRequest,
        message::{Message, ToolChoice, UserContent},
    };
    let messages = inject_tool_prompt(base_messages, &tools)?;
    let mut native = crate::providers::llm::text_request(&messages)?.chat_history;
    let prompt = native
        .pop()
        .ok_or_else(|| anyhow::anyhow!("tool run requires an initial prompt"))?;
    let max_depth = tools_config.max_tool_call_depth.max(1) as usize;
    let mut run = AgentRun::new(prompt)
        .with_history(native)
        .max_turns(max_depth + 1);
    let tool_map = tools
        .iter()
        .map(|tool| (tool.name.to_string(), tool.clone()))
        .collect::<HashMap<_, _>>();
    let admitted_turn = turn.ok_or_else(|| anyhow::anyhow!("tool loop has no original context"))?;
    let ctx = ToolContext {
        context_id: Some(admitted_turn.context_binding().await?.context_id),
        source_privacy_epoch: admitted_turn.source_privacy_epoch,
        user_id,
        source_received_at: admitted_turn.received_at,
        source_event_id: admitted_turn.source_event_id.get().copied(),
        request_id: trace.turn_id,
        invocation_key: String::new(),
        conversation_id: session_id,
        source: "ai".to_string(),
        channel_id: Some(channel_id.to_string()),
        cancel: Some(admitted_turn.token.clone()),
    };
    let mut used_tools = false;
    loop {
        if turn_is_interrupted(turn) {
            return Ok(ToolLoopOutcome {
                text: String::new(),
                used_tools,
            });
        }
        match run.next_step()? {
            AgentRunStep::CallModel {
                prompt,
                mut history,
                turn: round,
            } => {
                // Recheck the original context and every extension before every network attempt.
                super::context::validate_generation_context(modules, turn).await?;
                let mut spec = RunSpec::new();
                spec.tool_choice = llm.tool_choice().await?;
                if round > max_depth {
                    spec.tool_choice = Some(ToolChoice::None);
                    history.push(Message::system("Tool-call depth limit reached. Give a final natural-language answer using the existing receipts. Do not request further actions."));
                }
                let definitions = tools.iter().map(ToolDefinition::to_rig).collect::<Vec<_>>();
                let prepared = prepare_request(
                    &spec,
                    &Default::default(),
                    &history,
                    definitions,
                    None,
                    None,
                )?;
                run.advertise_tools(round, prepared.tools.clone());
                let request = prepared.clone().apply(CompletionRequest::new(prompt));
                let response = cancellable_stage(turn, "llm_tool_turn", async {
                    let deadline =
                        tokio::time::Instant::now() + crate::providers::llm::MODEL_TIMEOUT;
                    let mut stream = llm.completion_stream(request).await?;
                    // Stream once through Rig's fold. No parallel transcript or argument parser.
                    while let Some(item) = tokio::time::timeout_at(deadline, stream.next()).await? {
                        if matches!(item?, rig_core::streaming::Item::Unknown(_)) {
                            anyhow::bail!("unsupported provider stream event");
                        }
                    }
                    let response = tokio::time::timeout_at(deadline, stream.finish()).await??;
                    crate::providers::llm::validate_response(&response)?;
                    Ok(response)
                })
                .await?;
                let Some(response) = response else {
                    return Ok(ToolLoopOutcome {
                        text: String::new(),
                        used_tools,
                    });
                };
                // Defaults fail malformed/unknown tools and do not repair or retry calls.
                run.model_response(ModelTurn::from_response(&response, &prepared))?;
            }
            AgentRunStep::CallTools { calls } => {
                anyhow::ensure!(
                    run.turn() <= max_depth,
                    "tool dispatch exceeds the turn budget"
                );
                let mut results = Vec::with_capacity(calls.len());
                for (index, pending) in calls.into_iter().enumerate() {
                    anyhow::ensure!(
                        pending.preresolved_result.is_none(),
                        "automatic tool recovery is disabled"
                    );
                    if turn_is_interrupted(turn) {
                        return Ok(ToolLoopOutcome {
                            text: String::new(),
                            used_tools,
                        });
                    }
                    super::context::validate_generation_context(modules, turn).await?;
                    let native = pending.tool_call;
                    let call = ToolCall {
                        call_id: native.id.to_string(),
                        tool_name: native.function.name.to_string(),
                        arguments: serde_json::Value::Object(native.function.arguments),
                    };
                    debug_component_tool_calls(
                        debug_output,
                        run.turn() as u32,
                        std::slice::from_ref(&call),
                        Some((channel_id, user_id_text)),
                    );
                    used_tools = true;
                    // Never drop a dispatched Execution future on generation cancellation.
                    let mut call_context = ctx.clone();
                    call_context.invocation_key = format!("{}:{index}", run.turn() - 1);
                    call_context.context_id =
                        Some(admitted_turn.context_binding().await?.context_id);
                    let result = execute_turn_tool_call(
                        modules,
                        &call,
                        &tool_map,
                        &call_context,
                        admitted_turn,
                    )
                    .await?;
                    trace.record(modules, "tool_result", "tool", Some(call.tool_name.clone()), serde_json::json!({"tool":call.tool_name,"call_id":result.call_id,"success":result.success,"output":result.output})).await;
                    debug_component_tool_result(
                        debug_output,
                        &call.tool_name,
                        &result,
                        Some((channel_id, user_id_text)),
                    );
                    results.push(UserContent::ToolResult(rig_core::message::ToolResult {
                        call: native.id,
                        name: native.function.name,
                        content: result.to_rig_output().into_content(),
                        is_error: !result.success,
                    }));
                }
                run.tool_results(results)?;
            }
            AgentRunStep::Done(response) => {
                return Ok(ToolLoopOutcome {
                    text: response.output().to_string(),
                    used_tools,
                });
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct ToolSelection {
    pub available_tools: Vec<ToolDefinition>,
    pub selected_tools: Vec<ToolDefinition>,
    pub preferred_tool_names: Vec<String>,
}

pub(super) async fn select_tools_for_skill(
    modules: &InteractionServices,
    user_id: Uuid,
    preferred_names: &[String],
    user_text: &str,
    tools_config: &ToolsSection,
    channel_id: &str,
    assistant_name: &str,
) -> anyhow::Result<ToolSelection> {
    select_tools(
        modules,
        user_id,
        Some(preferred_names),
        user_text,
        tools_config,
        channel_id,
        assistant_name,
    )
    .await
}

pub(super) async fn select_all_tools(
    modules: &InteractionServices,
    user_id: Uuid,
    user_text: &str,
    tools_config: &ToolsSection,
    channel_id: &str,
    assistant_name: &str,
) -> anyhow::Result<ToolSelection> {
    select_tools(
        modules,
        user_id,
        None,
        user_text,
        tools_config,
        channel_id,
        assistant_name,
    )
    .await
}

async fn select_tools(
    modules: &InteractionServices,
    user_id: Uuid,
    preferred_names: Option<&[String]>,
    user_text: &str,
    tools_config: &ToolsSection,
    channel_id: &str,
    assistant_name: &str,
) -> anyhow::Result<ToolSelection> {
    let all_tools = modules.tools.list_tools_for(user_id).await?;
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
            tools.iter().map(|tool| tool.name.to_string()).collect()
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
    modules: &InteractionServices,
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
    modules: &InteractionServices,
    tools: &[ToolDefinition],
) -> HashMap<String, ToolRoutingIndexEntry> {
    if tools.is_empty() {
        return HashMap::new();
    }

    let tool_names = tools
        .iter()
        .map(|tool| tool.name.to_string())
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
        .filter(|tool| !index_by_name.contains_key(tool.name.as_str()))
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
    modules: &InteractionServices,
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
            embeddings_by_name.insert(tool.name.to_string(), stored.vector.clone());
            continue;
        }

        if tools_config.indexed_search_refresh_missing_embeddings {
            missing.push((tool.name.to_string(), routing.routing_text));
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
                .get(tool.name.as_str())
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
        name: candidate.tool.name.to_string(),
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
    if let Some(entry) = index_by_name.get(tool.name.as_str()) {
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

pub(super) async fn execute_turn_tool_call(
    modules: &InteractionServices,
    call: &ToolCall,
    tool_map: &HashMap<String, ToolDefinition>,
    ctx: &ToolContext,
    turn: &VoiceTurn,
) -> anyhow::Result<ToolResult> {
    let Some(tool) = tool_map.get(&call.tool_name) else {
        return Ok(ToolResult {
            presentation: None,
            call_id: call.call_id.clone(),
            success: false,
            output: serde_json::json!({"error":"tool_not_available"}),
            display: Some("The requested tool is not available.".into()),
        });
    };
    // Output admission and every tool extension share this gate. Hold it through
    // durable execution so preparation cannot freeze an incomplete read set.
    let mut state = turn.context.lock().await;
    anyhow::ensure!(
        !state.frozen,
        "tool completion cannot extend a frozen response"
    );
    let original = state
        .generation
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("tool completion has no original context"))?;
    anyhow::ensure!(
        ctx.context_id == Some(original.lease().id()),
        "tool invocation changed its original context"
    );
    let result = modules
        .tools
        .execute(
            ToolCall {
                tool_name: call.tool_name.clone(),
                arguments: call.arguments.clone(),
                call_id: call.call_id.clone(),
            },
            ctx,
        )
        .await?;
    let effect_id = result
        .output
        .get("effect_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(|| anyhow::anyhow!("tool result has no durable execution identity"))?;
    let (extended, canonical) = modules
        .context
        .extend_execution(original, effect_id)
        .await?;
    anyhow::ensure!(
        canonical
            .get("tool_name")
            .and_then(serde_json::Value::as_str)
            == Some(call.tool_name.as_str())
            && canonical
                .get("model_call_id")
                .and_then(serde_json::Value::as_str)
                == Some(call.call_id.as_str()),
        "canonical execution belongs to another tool invocation"
    );
    let status = canonical.get("status").and_then(serde_json::Value::as_str);
    let success = status == Some("succeeded");
    if original.policy().appraisal.affect.enabled
        && matches!(
            status,
            Some("succeeded" | "failed" | "unknown" | "cancelled")
        )
    {
        Box::pin(modules.expression.observe_execution_completion(
            &modules.context,
            &original.lease().binding(),
            effect_id,
        ))
        .await?;
        state.affect_dirty = true;
    }
    if success && call.tool_name == "memory_search" && tool.source == "builtin" {
        anyhow::ensure!(
            canonical["completion"]["status"] == "succeeded"
                && canonical["completion"]["receipt"]["success"] == true
                && canonical["completion"]["receipt"]["call_id"] == call.call_id
                && canonical["completion"]["receipt"]["output"]["context_read_requested"] == true,
            "memory search lacks a canonical successful read authorization"
        );
        let (query, opts) = memory_search_request(&call.arguments)?;
        let extended = modules
            .context
            .extend_memory_with_opts(&extended, query, opts)
            .await?;
        let result = ToolResult {
            presentation: None,
            call_id: call.call_id.clone(),
            success: true,
            output: serde_json::json!({
                "execution":canonical,
                "memory_context":extended.memory_prompt(),
            }),
            display: None,
        };
        state.generation = Some(extended);
        Box::pin(super::expression::project_turn_affect(modules, &mut state)).await?;
        return Ok(result);
    }
    let display = match status {
        Some("succeeded") => "Execution succeeded.",
        Some("failed") => "Execution definitively failed.",
        Some("cancelled") => "Execution was cancelled before dispatch.",
        Some("proposed") => "Authenticated confirmation is required before execution.",
        Some("approved" | "dispatched") => {
            "Execution is pending; do not claim completion or repeat it."
        }
        _ => "Execution outcome is unknown; inspect its record before any retry.",
    };
    state.generation = Some(extended);
    Box::pin(super::expression::project_turn_affect(modules, &mut state)).await?;
    Ok(ToolResult {
        presentation: ToolResult::canonical_presentation(&canonical),
        call_id: call.call_id.clone(),
        success,
        output: canonical,
        display: Some(display.into()),
    })
}

fn memory_search_request(
    arguments: &serde_json::Value,
) -> anyhow::Result<(&str, crate::modules::memory::RetrievalOpts)> {
    let query = arguments
        .get("query")
        .and_then(serde_json::Value::as_str)
        .filter(|query| !query.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("memory search requires a non-empty query"))?;
    let max_results = match arguments.get("max_results") {
        None => 5,
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=20).contains(value))
            .ok_or_else(|| anyhow::anyhow!("memory search max_results must be 1..=20"))?
            as u32,
    };
    let layers = arguments
        .get("layers")
        .map(|value| -> anyhow::Result<Vec<String>> {
            let values = value
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("memory search layers must be an array"))?;
            values
                .iter()
                .map(|value| {
                    let layer = value.as_str().ok_or_else(|| {
                        anyhow::anyhow!("memory search layers must contain strings")
                    })?;
                    anyhow::ensure!(
                        matches!(
                            layer,
                            "core"
                                | "episodic"
                                | "procedural"
                                | "preference"
                                | "relationship"
                                | "reflection"
                                | "project"
                                | "boundary"
                        ),
                        "unsupported memory search layer"
                    );
                    Ok(layer.to_string())
                })
                .collect()
        })
        .transpose()?;
    Ok((
        query,
        crate::modules::memory::RetrievalOpts {
            max_results: Some(max_results),
            min_similarity: None,
            layers,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition::new(
            name,
            description,
            serde_json::json!({
                "type":"object", "properties":{"query":{"type":"string"}}
            }),
            "builtin",
        )
        .unwrap()
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
