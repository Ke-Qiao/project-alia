# Memory Runtime

Alia treats memory as a built-in personal-agent runtime, not as a replaceable
provider slot. Memory decides what the assistant remembers, why it believes it,
and which evidence supports it. It does not directly modify affect state or
drive expression devices.

```text
completed turn / runtime event
  -> Memory Extractor
  -> Memory Policy
  -> canonical memory_items
  -> memory_embeddings_1536
  -> memory_nodes / memory_edges
  -> MemoryContextPack
```

## Storage

The production path uses PostgreSQL plus pgvector:

- `embedding_profiles` stores embedding-space metadata.
- `memory_items` is the canonical memory table.
- `memory_embeddings_1536` stores vectors for the active 1536-dimension profile.
- `memory_nodes` and `memory_edges` project lightweight graph context.

The default embedding profile is
`openai.text-embedding-3-small.1536.cosine.v1`. The 1536 dimension is isolated
to the profile and side table so future 1024 or 3072 profiles do not require
changing the canonical memory model.

If embeddings are unavailable, memory writes still succeed. Retrieval degrades
to keyword/metadata matching and `/api/v1/memories/capabilities` reports a
degraded embedding probe.

## Capture Modes

- `none`: memory stays available for direct API management, but the assistant
  does not capture conversation memories.
- `explicit_tool_only`: no automatic capture; the assistant stores or forgets
  memory only through explicit user-requested tools such as `memory_store` and
  `memory_forget`.
- `llm_selected`: recommended default for a personal assistant. A dedicated
  `memory_capture_llm`, when configured, extracts only durable facts,
  preferences, projects, boundaries, and relationship context before writing
  memory. If no dedicated model is configured, the minor LLM is used.

Recommended setup:

```toml
[policies.memory]
enabled = true
capture_mode = "llm_selected"
sensitive_filter_enabled = true
capture_min_importance = 0.45
active_embedding_profile = "openai.text-embedding-3-small.1536.cosine.v1"
graph_depth = 2
consolidation_enabled = true
paused_until = ""
retrieval_layers = ["core", "preference", "relationship", "project", "boundary"]
max_injected_memory_chars = 1200

[modules]
# Optional. Omit to fall back to minor_llm.
# memory_capture_llm = "gemini"
```

Sensitive chat-derived memories are blocked before storage. Credentials,
government or financial identifiers, health details, religious views, and
political views are only accepted through direct user-managed memory entry.

## Retrieval

Runtime retrieval returns a `MemoryContextPack`, not raw database rows:

- `user_preferences`
- `project_context`
- `relevant_episodes`
- `affective_guidance`
- `boundaries`
- `graph_context`
- `suppressed`
- `recall_events`

The pipeline injects these as separate prompt sections and includes a
`Do Not Mention` section that forbids exposing memory IDs, confidence scores,
affect scores, suppressed records, or runtime internals.

Hybrid retrieval combines vector search, keyword fallback, graph snippets,
policy memories, and reranking signals such as semantic similarity, recency,
salience, importance, confidence, graph relevance, and affect relevance.
`MemoryContextPack.retrieval_trace` records the component scores and retrieval
path for debugging and replay. Prompt injection ignores the trace and only uses
the safe snippet categories.

Graph projection is deliberately lightweight in v1. The runtime normalizes
explicit `topic_tags`, `metadata.topics`, `metadata.entities`, and
`metadata.devices`; it also derives coarse topics and entities from durable
memory content so graph retrieval works even before a dedicated extractor is
introduced.

## Affect And Expression

Memory emits `MemoryRecallEvent` records. Expression Runtime converts them into
bounded appraisal events before Affect Runtime updates state. Single recall
events are capped before reducers run: trust is capped at `0.02`, valence at
`0.05`, and engagement at `0.08`.

Expression planning reads only the context pack categories. Boundaries can cap
motion, style preferences can prefer direct or structured responses, project
context can bias toward focused technical expression, and affective guidance is
converted into calm constructive tone guidance. Raw memory scores are not
exposed to the natural-language generation layer.

## Consolidation

Online writes perform light reinforcement for exact repeated preferences,
boundaries, and projects. The runtime also exposes user consolidation for
deeper maintenance:

- deduplicate equivalent durable memories;
- promote preference-like episodes into preference memories;
- demote low-confidence or low-importance memories;
- supersede or mark contradictory preferences;
- reframe negative affective memories into constructive guidance.

## Runtime APIs

- `GET /api/v1/memories`
- `POST /api/v1/memories`
- `DELETE /api/v1/memories`
- `GET /api/v1/memories/{id}`
- `PATCH /api/v1/memories/{id}`
- `DELETE /api/v1/memories/{id}`
- `GET /api/v1/memories/{id}/explain`
- `POST /api/v1/memories/search`
- `POST /api/v1/memories/search/context`
- `GET /api/v1/memories/graph`
- `DELETE /api/v1/memories/topic/{topic}`
- `DELETE /api/v1/memories/kind/{kind}`
- `DELETE /api/v1/memories/hidden`
- `GET /api/v1/memories/{id}/suppressed`
- `GET /api/v1/memories/export`
- `POST /api/v1/memories/consolidate`
- `GET /api/v1/memories/privacy`
- `PATCH /api/v1/memories/privacy`
- `GET /api/v1/memories/audit`
- `GET /api/v1/memories/capabilities`

`/api/v1/memories/export` returns user-visible memories and suppressed reasons
for hidden private-runtime records. `/api/v1/memories/search/context` returns
the full context pack for debugging UI and user privacy views.

`PATCH /api/v1/memories/privacy` accepts `pause_days` and `clear_pause` in
addition to `enabled`, `automatic_capture_enabled`, and `capture_mode`. Pausing
memory disables automatic capture and runtime retrieval until `paused_until`;
direct user-managed memory APIs remain available so users can inspect or delete
stored records.
