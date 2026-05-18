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

## Memory and Insights

Memory is provider-backed. Mem0 is the main external target; `simple` is kept as
a minimal fallback for local development and tests. If memory health checks fail
at startup, memory should be disabled for prompt assembly instead of advertised
to the LLM.

Insights are background analysis hooks. They can later improve memory, cache
entries, tool metadata, and user-facing summaries during idle time.

## Cache

The cache is intended for safe, memory-independent, tool-independent responses.
Dynamic turns that depend on tools, current sensor values, personal memory, or
fresh external state should not be cached as direct final answers.

## Users and Auth

The server owns user identities, auth, and frontend-facing account APIs. The
voice pipeline must keep user ids explicit so memory, conversation history,
settings, tools, and future voiceprint identification can remain multi-user
safe.

## Agent History

Agent history is separate from chat transcript messages. It records route
decisions, cache checks, LLM calls, tool calls/results, memory/cache events, and
delivery events so the frontend can show how a turn was handled.

