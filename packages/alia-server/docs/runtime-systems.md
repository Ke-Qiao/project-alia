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
The `tools.routing_keywords` and `tools.routing_text` fields are the primary
lexical source of truth; Rust only keeps generic structural boosts such as URL
inputs for `fetch` and arithmetic expressions for `calculate`. When an embedding
provider is available, the pipeline lazily stores routing embeddings for that
metadata and adds semantic similarity before optional minor LLM filtering.

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
- Cron accepts both 5-field Unix expressions and 6-field expressions with an
  explicit seconds column.
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

For audio turns, authenticated continuous capture is the sole input path:

```text
VoiceSession v2: ordered capture + local VAD evidence
  -> server Easy-Turn provider
  -> owner-bound recognized voice input
  -> router/cache/appraisal/memory
  -> registered Playback v3 / BodyControl output
```

The original account, channel, device boot, session, generation, and source
epochs constrain admission and downstream context. There is no second local
utterance/STT endpoint or transcript-correction LLM role. The server processes
the accepted recognized transcript directly. Session-owned cancellation
supersedes old work and preserves the existing persistence/context safeguards;
unused channel interrupt policy switches have been removed.

The old `end_continuous_dialogue` tool has been deleted. A future explicit
input-session close action needs an authenticated owner and acknowledged capture
closure; Playback Fence alone is only output admission control.

## Agent History

Agent history is separate from chat transcript messages. It records route
decisions, cache checks, LLM calls, tool calls/results, memory/cache events, and
delivery events so the frontend can show how a turn was handled.
