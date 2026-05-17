# Modules and Providers

Project Alia uses module slots and provider traits to keep the runtime
replaceable. The server decides which implementation fills each slot, while the
pipeline depends only on trait interfaces.

## Terms

| Term | Meaning |
| --- | --- |
| Module slot | A named runtime capability such as `llm`, `stt`, `tts`, `memory`, or `tools`. |
| Provider | A concrete implementation behind a slot, such as OpenAI-compatible LLM, Ollama, whisper.cpp, GPT-SoVITS, or Mem0. |
| Channel | A multi-instance I/O path, such as embedded gRPC or CLI. |
| Extension | Future plugin/component code that can register tools, skills, settings, conditions, or module-like behavior. |
| Lifecycle | Shared initialization, health check, runtime setting application, and shutdown contract. |

## Server Slots

Server slots are selected in `[modules]`. Provider configuration is split into
fragment files under `server.d/`.

This keeps these decisions separate:

- "Which implementation should run?" belongs in `[modules]`.
- "How is that implementation configured?" belongs in provider/policy/profile
  fragments.

## Provider Traits

The server defines traits for LLM, STT, TTS, embeddings, memory, cache, skills,
persona, conversation history, insights, router, rules, scheduler, tools, and
tool execution.

Provider implementations should be boring adapters: translate Alia's internal
types into provider calls, return normalized results, and avoid owning pipeline
policy.

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
- LLM providers: `llm-openai`, `llm-ollama`
- speech providers: `stt-whisper`, `tts-sovits`
- embeddings: `embed-ollama`, `embed-openai`
- memory: `memory-mem0`
- cache: `cache-semantic`
- built-in HTTP tools: `tools-http`
- insights: `insights`

Embedded features currently cover sherpa-onnx wake word, edge STT, and direct
responses.

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
