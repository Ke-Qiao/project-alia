# Model Services

Project Alia treats models as replaceable services behind provider adapters.
This document lists the current service roles without going deep into provider
installation details.

## Embedded Models

Embedded uses sherpa-onnx for local audio intelligence:

- wake word keyword spotting
- Silero VAD activity evidence for the continuous capture stream

Model paths are configured in `config/embedded.toml`. The helper script
`scripts/download-sherpa-models.sh` populates the KWS and Silero VAD model
directories. Embedded does not run local STT or choose turn boundaries.

Embedded audio/model behavior is described in
[alia-embedded audio and voice](../../packages/alia-embedded/docs/audio-and-voice.md).

## Server Turn Detection

Easy-Turn runs only on the Server host in the current macOS Server + Pi 5
Embedded deployment. Embedded keeps wake word, VAD, continuous capture and
playback; it does not host this model. Enable it in
`config/server.d/providers/easy_turn.toml`:

```toml
[providers.easy_turn]
enabled = true
endpoint = "http://127.0.0.1:8091"
timeout_ms = 5000
max_concurrency = 1
```

Start the pinned model service before enabling Embedded `[speech].enabled`.
Continuous VoiceSession is the only device voice path. Preparation, pinned
revisions and the service HTTP
contract are documented in the [Easy-Turn service guide](../../services/easy-turn/README.md).
`/health` confirms readiness and `/v1/capabilities` reports the actual model,
format and concurrency limits. Readiness alone does not measure first-request
latency; record initial and subsequent calls separately when assessing load.

Server uses bounded PCM windows to obtain a transcript and Complete,
Incomplete, Backchannel or WAIT evidence. This is continuous capture with
windowed inference, not a model-native incremental decoder. The accepted model
transcript goes directly into the recognized voice input
path and is not sent through a second STT request.

Use matching voice v2 and playback v3 peers. A fresh Backchannel may release a
temporary acoustic hold; explicit WAIT retains the same output until explicit
Continue. The 3-second temporary semantic deadline, configurable 60-second
hold default (hard maximum 120 seconds), and original output lifetime are
separate from `timeout_ms`. Increasing a model HTTP timeout does not extend
resume authority. These are engineering defaults; see the
[voice protocol](../refactor/voice-protocol.md) for exact ownership and expiry
rules. Model labels and `inference_ms` do not prove physical speaker pause,
AEC or end-to-end acoustic latency.

## Server STT

Server STT currently uses a whisper.cpp-compatible HTTP service. The server
provider is configured under:

```text
config/server.d/providers/whisper_cpp.toml
```

The authenticated HTTP voice/transcription APIs can use this provider for
explicit audio requests. Device VoiceSession uses the Easy-Turn transcript
described above and does not issue a second STT request.

## Server LLM

LLM providers are split into major and minor roles:

- minor LLM is usually a smaller/cheaper model and can be used for routing or lighter work
- major LLM is usually OpenAI-compatible and can also point at LM Studio if it
  exposes an OpenAI-compatible API

Configure these under:

```text
config/server.d/providers/openai.toml
config/server.d/providers/gemini.toml
config/server.d/providers/anthropic.toml
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

Memory is an Alia-owned core system. Capture policy, privacy filtering,
relationship modeling, and scheduled memory meditation live in the server
runtime instead of a replaceable provider adapter. Configure memory policy
under:

```text
config/server.d/policies/memory.toml
```

Automatic memory capture is agent-policy driven by default. `capture_mode =
"llm_selected"` asks the assistant-side LLM to extract only durable user facts,
preferences, stable relationship context, goals, or habits before storing
Alia `MemoryNode`s. External services do not decide what should be remembered.

If the built-in memory system fails startup health checks, memory should be
disabled for prompt assembly rather than advertised to the LLM.

Memory privacy controls are available through `/api/v1/memories/privacy`.
Setting `automatic_capture_enabled = false` switches capture to
`explicit_tool_only`, while the memory system remains available for explicit
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
