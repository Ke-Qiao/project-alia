# Model Services

Project Alia treats models as replaceable services behind provider adapters.
This document lists the current service roles without going deep into provider
installation details.

## Embedded Models

Embedded uses sherpa-onnx for local audio intelligence:

- wake word keyword spotting
- edge STT for local transcript/debug behavior
- Silero VAD through sherpa-onnx for endpointing

Model paths are configured in `config/embedded.toml`. The helper script
`scripts/download-sherpa-models.sh` is intended to populate the common local
model directories.

Embedded audio/model behavior is described in
[alia-embedded audio and voice](../../packages/alia-embedded/docs/audio-and-voice.md).

## Server STT

Server STT currently uses a whisper.cpp-compatible HTTP service. The server
provider is configured under:

```text
config/server.d/providers/whisper_cpp.toml
```

The server sends captured utterance audio to this service and receives text back
before routing the turn through the LLM/tool pipeline.

## Server LLM

LLM providers are split into major and minor roles:

- minor LLM is usually a smaller/cheaper model and can be used for routing or lighter work
- major LLM is usually OpenAI-compatible and can also point at LM Studio if it
  exposes an OpenAI-compatible API

Configure these under:

```text
config/server.d/providers/openai.toml
config/server.d/providers/ollama.toml
config/server.d/policies/routing.toml
```

## Server TTS

Server TTS currently targets the GPT-SoVITS WebAPI. Provider endpoint and timeout
settings live under:

```text
config/server.d/providers/gpt_sovits.toml
```

Voice profile and reference-audio parameters live under:

```text
config/server.d/profiles/voice.default.toml
```

Files referenced by GPT-SoVITS config are paths on the remote TTS server, not
local files inside `alia-server`.

## Memory

Memory is provider-backed. The primary external target is Mem0; `simple` exists
as a minimal fallback for local use and tests. Configure Mem0 under:

```text
config/server.d/providers/mem0.toml
config/server.d/policies/memory.toml
```

Automatic memory capture is agent-policy driven by default. `capture_mode =
"llm_selected"` asks the assistant-side LLM to extract only durable user facts,
preferences, stable relationship context, goals, or habits before storing
provider-neutral `MemoryNode`s. Mem0 is used as storage/retrieval, not as the
default judge for every conversation turn. Use `capture_mode =
"provider_infer_all"` only when you intentionally want the memory provider to
infer from every completed turn.

If the configured memory backend fails startup health checks, the memory module
should be disabled for prompt assembly rather than advertised to the LLM.

Memory privacy controls are available through `/api/v1/memories/privacy`.
Setting `automatic_capture_enabled = false` switches capture to
`explicit_tool_only`, while the memory backend remains available for explicit
user actions such as the memory UI/API or `memory_store` / `memory_forget`.
Automatic and chat-tool memory writes block sensitive categories by default
(credentials, identifiers, health details, religious views, and political
views). Sensitive memories are only accepted through direct user-managed memory
entry.

Prompt injection uses only configured retrieval layers, defaults to
`core`, `preference`, and `relationship`, strips metadata, filters sensitive
content, and applies a bounded character budget before adding memories to the
runtime prompt.

## Deployment Reminder

The server expects provider endpoints to already be reachable. Model download,
container startup, GPU/Metal tuning, and provider-specific performance settings
should live in setup scripts or provider deployment docs rather than in the core
architecture docs.

Server provider configuration is described in
[alia-server configuration](../../packages/alia-server/docs/configuration.md).
Memory capture modes and provider-neutral capabilities are described in
[memory operations](./memory.md).
