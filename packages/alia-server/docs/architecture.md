# Server Architecture

`alia-server` is the coordinator. It receives channel events, routes turns
through the AI pipeline, calls model providers and tools, persists state, and
exposes APIs for the frontend and future plugins.

## Main Layers

| Layer | Responsibility |
| --- | --- |
| API | Axum routes under `/api/v1`, `/health`, OpenAPI/Scalar, OOBE/bootstrap, settings, devices, users, auth, sensors, rules, scheduler, tools, skills, and conversations. |
| Channels | Multi-instance I/O adapters such as embedded gRPC and CLI. Channels translate user/device I/O into pipeline events and deliver responses. |
| Pipeline | Fixed turn orchestration: conversation context, routing, cache policy, memory snippets, skills, tools, LLM/tool loop, TTS, delivery, persistence, and agent trace events. |
| Providers | Swappable adapters for LLM, STT, TTS, embeddings, memory, and other edge execution backends. |
| Registries | Fixed runtime access to modules, channels, settings, tools, skills, dependency ownership, scheduler conditions/actions, and sensors. Plugins extend these registries rather than replacing them. |
| Persistence | PostgreSQL via SeaORM for users, sessions, messages, agent events, tools, skills, cache, personas, rules, scheduler tasks, settings, and sensor data. |

## Provider Selection And Core Services

The `[modules]` section in `config/server.toml` selects provider adapters only.
Core services are constructed by the server builder and expose extension
registries instead of replacement slots.

| Provider key | Typical implementation | Purpose |
| --- | --- | --- |
| `minor_llm` | `ollama` | Lightweight model for routing, tool narrowing, and simple work. |
| `major_llm` | `openai` | Stronger model for complex turns. |
| `stt` | `whisper_cpp` | Server-side speech recognition. |
| `tts` | `gpt_sovits` | Server-side speech synthesis. |
| `embedding` | `ollama` or `openai` | Embeddings for semantic cache and retrieval. |
| `memory` | `mem0` or `simple` | User memory backend behind the fixed memory integration boundary. |

These services are fixed core runtime pieces:

- pipeline and turn state
- conversation/history persistence
- cache storage and cache policy
- skill registry and skill/tool bindings
- persona rendering and runtime prompt assembly
- insights scheduler/strategy wiring
- router orchestration and tool candidate filtering
- rule engine and automation scheduler
- tool registry and code-backed tool execution
- settings, sensor, channel, and dependency registries

## Pipeline Flow

```mermaid
sequenceDiagram
    participant Channel
    participant Pipeline
    participant Router
    participant Tools
    participant LLM
    participant TTS

    Channel->>Pipeline: ChannelEvent
    Pipeline->>Pipeline: Load user, session, persona, memory, context
    Pipeline->>Router: Route turn
    Router-->>Pipeline: cache/minor/major/skill decision
    Pipeline->>LLM: Prompt + selected tools
    LLM-->>Pipeline: Text and/or tool calls
    Pipeline->>Tools: Execute calls
    Tools-->>Pipeline: Tool results
    Pipeline->>LLM: Continue loop when needed
    Pipeline->>TTS: Synthesize voice response
    Pipeline-->>Channel: Text/audio response
```

The pipeline records traceable agent events separately from chat messages so a
frontend can inspect route decisions, tool calls, tool results, cache behavior,
and delivery steps.

## Failure Model

Startup failures should disable only the affected module or channel when
possible. Runtime turn failures are logged and should not disable a channel by
default. Provider failures should degrade the specific path, not panic the whole
process.

## Plugin Direction

Future plugins should use the same registries as built-ins:

- register tools, skills, settings, sensors, conditions, and actions
- register dependency ownership for safe uninstall
- expose health and restart requirements
- avoid private side registries that the frontend, scheduler, or tools cannot see
