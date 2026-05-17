# Server Architecture

`alia-server` is the central runtime. It exposes the HTTP/WebSocket API, hosts
the AI pipeline, manages persistent state, and connects channels to model
providers.

## Main Layers

| Layer | Responsibility |
| --- | --- |
| API | Axum routes under `/api/v1`, plus `/health`, OpenAPI/Scalar support, and OOBE/bootstrap endpoints. |
| Channels | Input/output adapters such as embedded gRPC and CLI. Channels translate user/device I/O into pipeline events. |
| Pipeline | The request-turn orchestrator: conversation context, routing, cache, memory, skills, tools, LLM, TTS, and response delivery. |
| Providers | Swappable adapters for LLM, STT, TTS, embeddings, memory, cache, skills, persona, routing, rules, scheduler, and tools. |
| Module lifecycle | Startup, health checks, runtime setting application, and graceful shutdown. |
| Registries | Runtime access to modules, channels, settings, tools, skills, dependencies, and scheduler conditions. |
| Persistence | PostgreSQL via SeaORM for users, sessions, messages, agent events, skills, tools, cache, personas, rules, and registry ownership. |

## Module Graph

The server has single-instance module slots selected by `[modules]` in
`config/server.toml`. Current slots include:

| Slot | Typical implementation | Purpose |
| --- | --- | --- |
| `llm` | `openai` | Default LLM adapter. |
| `local_llm` | `ollama` | Lightweight/local model used by routing and simple responses. |
| `remote_llm` | `openai` | Larger model used for complex turns. |
| `stt` | `whisper_cpp` | Server-side speech recognition. |
| `tts` | `gpt_sovits` | Server-side speech synthesis. |
| `embedding` | `ollama` or `openai` | Embeddings for semantic cache and retrieval. |
| `memory` | `mem0` or `simple` | User memory backend. |
| `cache` | `two_layer` | Exact and semantic response cache. |
| `skills` | `builtin` | Skill registry and intent matching. |
| `persona` | `template` | User/persona prompt rendering. |
| `insights` | `noop` or scheduled | Background analysis and memory/cache improvement hooks. |
| `router` | `multi_layer` | Local/remote/cache/skill routing decision. |
| `rule_engine` | `automation` | Event-driven automation rules. |
| `scheduler` | `automation` | Cron, one-shot, interval, condition polling, and event subscriptions. |
| `tools` | `builtin` | Tool registry and code-backed execution. |

The lifecycle interface lets modules initialize, start, stop, report health, and
apply runtime settings. Providers should keep business logic behind trait
interfaces so plugin implementations can replace them later.

## Channels

Channels are multi-instance I/O adapters. They do not own assistant business
logic; they describe capabilities and deliver events/responses.

Current channel types:

- `embedded`: gRPC connection to an edge node, with voice input/output,
  streaming audio, playback stop, hardware/device actions, and channel-local
  interruption policy.
- `cli`: local text channel for development and debugging.

Each channel can opt into its own interruption behavior. This keeps future
plugin channels free to define different safety and playback semantics.

## Pipeline

The pipeline processes one turn at a time:

1. Receive a channel event.
2. Load active conversation/session context.
3. Add internal system context, persona, memory snippets, and channel context.
4. Check cache when policy allows.
5. Route to skill, local LLM, or remote LLM.
6. Select the tool set and run the LLM/tool loop.
7. Persist traceable agent history.
8. Deliver text and, for voice channels, TTS audio.
9. Store conversation and optionally ingest memory.

The pipeline records agent events separately from normal chat messages so a
frontend can later show what happened inside a turn, including route decisions,
tool calls, tool results, cache behavior, and delivery steps.

## Failure Model

Startup failures should disable the affected module or channel where possible.
Runtime turn failures are logged and should not disable a channel by default.
Embedded-specific errors such as failed playback or unavailable direct response
audio should degrade that response path without taking the whole process down.

## Automation

Rules and scheduler tasks share the same condition and action registries:

- `rules` listen to `ServerEvent` and run actions when an event pattern plus
  conditions match.
- `scheduler` owns cron, one-shot, interval, condition-polling tasks, and event
  subscriptions.
- Conditions can be built-in, such as sensor, time range, event field, and event
  pattern checks, or plugin-defined through the shared condition registry.
- Actions can log, emit an event, call a tool, or be plugin-defined through the
  shared action registry.
- Rules, tasks, and event subscriptions are persisted in PostgreSQL and loaded
  at server startup.
