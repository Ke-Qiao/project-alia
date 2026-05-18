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
narrow candidates before optional local LLM filtering.

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

Memory is provider-backed. Mem0 is the main external target; `simple` is kept as
a minimal fallback for local development and tests. If memory health checks fail
at startup, memory should be disabled for prompt assembly instead of advertised
to the LLM.

Insights are background analysis hooks. Scheduled insights can ingest recent
conversation into the memory backend, ask an LLM to extract durable memories,
consolidate existing memories, reflect on stable user context, detect repeated
habits, and review pending cache entries before enabling or deleting them. If
LLM or memory is unavailable, each phase degrades to a logged skip instead of
crashing the server.

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

## Agent History

Agent history is separate from chat transcript messages. It records route
decisions, cache checks, LLM calls, tool calls/results, memory/cache events, and
delivery events so the frontend can show how a turn was handled.
