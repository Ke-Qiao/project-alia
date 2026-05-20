# Runtime Systems

This page collects server subsystems that are used across the pipeline and
plugin model.

## Skills and Tools

Tools are atomic execution units. Skills are cognitive context. A skill can
contribute prompt text and bind a scoped set of tools, but it should not behave
like a giant function call.

Every built-in, component-owned, or plugin-owned tool/skill should be registered
in the database catalog. The runtime dependency tree tracks ownership so removal
can be safe.

Tool candidate selection uses the persisted tool catalog as a routing index.
When an embedding provider is available, the pipeline lazily stores routing
embeddings for tool metadata and uses semantic similarity plus lexical hints to
narrow candidates before optional minor LLM filtering.

## Tool Calling

The preferred path is native provider tool/function calling. Text-based tool
call parsing is fallback-only and accepts strict structured envelopes. The
pipeline should ignore examples or ordinary assistant prose that merely looks
like tool syntax.

## Automation

Rules and scheduler tasks share condition and action registries.

- Rules are event-driven.
- Scheduler tasks can be cron, one-shot, interval, condition-polling, or
  event-subscription based.
- Conditions include sensor, time range, event field, event pattern, and future
  plugin-defined evaluators.
- Actions include log, emit event, tool call, insights hooks, and future
  plugin-defined executors.

Sensor conditions use the shared sensor freshness policy. By default they ask
the source reader for a fresh value when the cache is missing or old, and fail
closed if only stale data is available. Individual conditions can opt into
`cache_only`, `best_effort`, custom `max_age_sec`, and custom `timeout_ms`.

## Memory and Insights

Memory is a built-in Alia runtime, parallel to Affect and Expression. It owns
what the assistant remembers, why it believes it, and which evidence supports
it. The runtime uses PostgreSQL canonical records, pgvector embedding side
tables, and PostgreSQL graph edge tables:

```text
Observation
  -> Memory Extractor
  -> Memory Policy / Dedup / Privacy
  -> Memory Store
  -> Graph Projection
  -> Hybrid Retrieval
  -> MemoryContextPack
```

Prompt assembly consumes `MemoryContextPack` sections, not raw memory rows:
preferences, projects, boundaries, affective guidance, relevant episodes, graph
context, and do-not-mention rules. Suppressed or private-runtime memories are
reported as hidden reasons for UI/debug views but are not injected into LLM
prompts.

The context pack also carries retrieval trace data for server-side replay:
retrieval path, component scores, graph relevance, affect relevance, and the
reason a memory was selected. This trace is for API/debug use only and is not
rendered into natural-language prompts.

Memory does not directly modify affect state. It emits `MemoryRecallEvent`
records, and Expression Runtime converts those into bounded appraisal events
before Affect Runtime reducers run. Expression planning reads only style
preferences, boundaries, project context, and constructive affective guidance.

Insights are background analysis hooks. Scheduled insights can ingest recent
conversation into the memory system, ask an LLM to extract durable memories,
consolidate existing memories, reflect on stable user context, detect repeated
habits, and review pending cache entries before enabling or deleting them. If
LLM or memory is unavailable, each phase degrades to a logged skip instead of
crashing the server.

The Memory Runtime also exposes explicit consolidation and user-control APIs for
dedupe, promotion, demotion, superseding, contradiction handling, topic/kind
deletion, hidden/private-runtime deletion, export, hidden reason inspection,
pause/resume controls, and privacy-mode updates.

## Cache

The cache is intended for safe, memory-independent, tool-independent responses.
Dynamic turns that depend on tools, current sensor values, personal memory, or
fresh external state should not be cached as direct final answers.

Pipeline writes create disabled pending entries. Only insights can promote an
entry to active direct return after LLM review and optional wording refinement.
Keyword exclusion lists live in `server.d/resources/cache_policy.toml`.

## Users and Auth

The server owns user identities, auth, and frontend-facing account APIs. The
voice pipeline must keep user ids explicit so memory, conversation history,
settings, tools, and future voiceprint identification can remain multi-user
safe.

For audio turns, raw STT output is normalized before it enters routing,
appraisal, cache, conversation history, or memory ingest:

```text
AudioInput
  -> STT raw transcript
  -> transcript correction
  -> corrected user text
  -> router/cache/appraisal/memory
```

The correction stage uses a structured JSON prompt and is conservative: it may
fix likely recognition errors, punctuation, and homophones, but it must not
answer the user or change intent. Risky or ambiguous corrections keep the raw
text for processing and are recorded in agent history. This means the memory
system receives the same corrected user text that downstream routing sees,
while the raw transcript remains available only in trace/debug events.

Deployments may configure an optional `modules.transcript_correction_llm`
backend for this stage. It is disabled by default; when absent, the default
correction provider falls back to the configured minor LLM and then major LLM.
Set `[policies.transcript_correction].provider = "transcript_correction_llm"`
to require the dedicated backend, or `provider = "none"` to trace no correction
model at all.

## Agent History

Agent history is separate from chat transcript messages. It records route
decisions, cache checks, LLM calls, tool calls/results, memory/cache events, and
delivery events so the frontend can show how a turn was handled.
