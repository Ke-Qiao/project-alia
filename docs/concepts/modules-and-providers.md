# Modules and Providers

Project Alia separates fixed core services from replaceable provider adapters.
The server owns one coherent runtime kernel; plugins and providers extend that
kernel through registries instead of replacing the registries themselves.

Package-specific module details live in:

- [alia-server architecture](../../packages/alia-server/docs/architecture.md)
- [alia-embedded architecture](../../packages/alia-embedded/docs/architecture.md)

## Terms

| Term | Meaning |
| --- | --- |
| Core service | A fixed runtime subsystem with shared invariants, such as pipeline orchestration, tools, skills, rules, scheduler, settings, sensors, and dependency ownership. |
| Provider | A replaceable adapter selected by config, such as OpenAI-compatible LLM, Anthropic, Gemini, whisper.cpp, or GPT-SoVITS. |
| Channel | A multi-instance I/O path, such as embedded gRPC or CLI. |
| Extension | Plugin/component code that registers tools, skills, settings, conditions, actions, sensors, prompt/context sections, or health checks. |
| Lifecycle | Shared initialization, health check, runtime setting application, and shutdown contract. |

## Provider Selection

Only provider adapters are selected in `[modules]`. Provider configuration is
split into fragment files under `server.d/`.

This keeps these decisions separate:

- "Which external/provider adapter should run?" belongs in `[modules]`.
- "How is that implementation configured?" belongs in provider/policy/profile
  fragments.
- "How do tools, skills, scheduler conditions, rules, sensors, and settings
  compose?" belongs in the fixed core registries.

## Provider Traits

The server defines provider traits for LLM, STT, TTS, embeddings, and tool
execution.

Provider implementations should be boring adapters: translate Alia's internal
types into provider calls, return normalized results, and avoid owning pipeline
policy.

Core services still use traits internally where it helps testing and lifecycle
management, but they are not public replacement slots. For example, plugins add
tools to the shared tool registry; they do not swap out the registry.

## Channels Are Not Providers

Channels are I/O paths, not model providers. A channel owns transport details
and local interaction behavior:

- supported capabilities
- text/audio delivery
- hardware/channel actions
- interruption behavior
- playback stop behavior

The embedded channel can therefore implement voice-specific semantics without
making the global pipeline assume every channel is a speaker/microphone device.

## Feature Flags

Cargo features compile optional backend support:

- server channel support: `channel-embedded`
- LLM providers: `llm-openai`, `llm-anthropic`, `llm-gemini`
- speech providers: `stt-whisper`, `tts-sovits`
- embeddings: `embed-openai`
- built-in HTTP tools: `tools-http`
- insights: `insights`

Embedded features currently cover sherpa-onnx wake word, edge STT, VAD, direct
responses, audio I/O, settings, and sensor service integration.

## Plugin Direction

The future plugin model should use the same abstractions instead of inventing a
parallel system:

- register settings under `plugin.<name>.*`
- register tools with code-backed executors
- register skills with explicit tool bindings
- register automation conditions through the shared condition registry
- register automation actions through the shared action registry
- register dependency ownership for safe uninstall
- expose health and restart requirements when runtime updates are unsafe

That keeps built-in components and plugins visible to the same dependency tree.
