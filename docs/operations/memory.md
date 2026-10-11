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
  -> memory_embeddings (profile + canonical revision + input digest)
  -> memory_nodes / memory_edges
  -> MemoryContextPack
```

## Storage

The production path uses PostgreSQL plus pgvector:

- `embedding_profiles` stores immutable descriptors of the actual embedding
  request: provider, endpoint, model, dimensions, distance, and vector type.
- `memory_items` is the canonical memory table.
- `memory_embeddings` stores projections with their canonical revision, exact
  input digest, policy version, privacy epoch, and actual profile identity.
- `memory_nodes` and `memory_edges` project lightweight graph context.

There is no handwritten profile alias or fixed-dimension Memory table. The
configured adapter previews its actual request descriptor; the persistent
Context policy pins that descriptor's digest. A missing or mismatched pin uses
keyword retrieval. A startup configuration change cannot silently replace the
database policy; explicitly applying the embedding provider settings performs
the policy transition. The displayed Memory profile is read-only. Cache and
Memory use the same descriptor format but own separate projections and policy
gates; Memory retrieval does not require Cache to be enabled.

If embeddings are unavailable, memory writes still succeed. Retrieval degrades
to keyword/metadata matching and `/api/v1/memories/capabilities` reports a
degraded embedding probe.

Projection work is prepared in the canonical write transaction. It keeps that
revision, input digest, policy version, privacy epoch, and expected descriptor
through inference, and checks them both before the request and in the result
transaction. Updating a Memory row removes its old vectors in the same
transaction. A delayed result cannot overwrite a new revision, recreate a
deleted record, or acquire a new policy identity by retrying.

One owned worker rebuilds missing projections from canonical records in keyset
batches of 16. Failed rows do not block later rows; another sweep retries them.
Inline and background work share two inference permits. Interactive inference
has a two-second budget, background inference 30 seconds, and database phases
two seconds. Shutdown cancels requests and waits for the owned worker and all
permits before releasing dependencies. Sensitive and private-runtime records
are excluded from external embedding. See the [projection contract](../refactor/memory-embedding-design.md)
for the validation boundary and test evidence.

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
graph_depth = 2
consolidation_enabled = true
paused_until = ""
retrieval_layers = ["core", "preference", "relationship", "project", "boundary"]
max_injected_memory_chars = 1200

[llm.roles]
# Optional. Omit to fall back to the minor role.
# memory_capture = "my-configured-model"
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

Privacy settings are global SQL policy. Reading them requires authentication;
changing them requires an administrator, including through this older privacy
endpoint. Both privacy reads and `audit.privacy` report the committed policy,
not a process-local settings projection. A successful PATCH survives restart
without writing `settings.toml`. It accepts the same quoted policy-version
`If-Match` as `/api/v1/settings`; obtain the version from the settings GET ETag.
A stale version returns 409, and a failed or timed-out policy operation must be
read back before retrying. The existing privacy response fields are unchanged.
