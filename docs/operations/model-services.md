# Model Services

Project Alia treats models as replaceable services behind provider adapters.
This document lists the current service roles without going deep into provider
installation details.

The [software release record](software-release.md) separates verified binaries
and noop lifecycle evidence from model deployment information that still needs
confirmation. Example URLs and model names below are configuration examples,
not evidence that a service is installed or permission to start or probe it.

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

There is no Embedded `speech.enabled` switch. For a later authorized device
deployment, select the capture/playback, wake-word and VAD modules, configure
`speech.channel_id`, and satisfy the normal binding/session requirements. Keep
the all-noop staging profile for preparation without devices; choosing real
audio modules can open devices during startup. Continuous VoiceSession is the
only device voice path. Preparation, pinned
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
described above and does not issue a second STT request. The Whisper endpoint
is a base URL to which the adapter appends `/inference` and `/health`; its
configured `model` is informational, while the actual weights are selected by
the service startup configuration.

## Server LLM

Chat roles select a model profile in `profiles.llm.<id>`, which selects a named
adapter in `providers.llm.<id>`. Major and minor are the main chat roles:

- minor LLM is usually a smaller/cheaper model and can be used for routing or lighter work
- major LLM is usually OpenAI-compatible and can also point at LM Studio if it
  exposes an OpenAI-compatible API

Record the actual provider ID, model ID/version, complete base URL, API family
and credential source for each selected role. OpenAI adapters support
`chat_completions` or `responses`; the chosen service must support that exact
family and the required streaming, tool and JSON behavior. Anthropic and Gemini
use their own adapters. An OpenAI-compatible label alone does not establish
these capabilities.

`default` currently reuses minor when its profile equals minor, otherwise
major; it does not create an independent third chat model. An omitted
`memory_capture` role uses the minor fallback. An omitted `sentence_expression`
role uses deterministic expression rules. Dedicated optional roles must be
included in the deployment inventory: a local major model does not make other
roles local. Do not change capture policy or execution approval to simplify a
model test.

Configure these under:

```text
config/server.d/providers/openai.toml
config/server.d/providers/gemini.toml
config/server.d/providers/anthropic.toml
config/server.d/policies/routing.toml
```

Model profiles are in `config/server.d/profiles/llm.toml`. The repository example
includes remote providers for some roles; it is not an approved deployment.

## Server Embedding

The embedding role is independent of chat roles. The current real adapter uses
an OpenAI-compatible `/embeddings` endpoint. Confirm its provider, model ID,
actual vector dimensions, service/weight version and credential source. Only
`text-embedding-3*` requests send a dimensions parameter; other models must
already return the configured dimension. Response identity, ordering and vector
shape are checked, but the request descriptor's `request-config.v1` revision is
not proof of remote weight identity.

Memory and cache bind semantic use to the active profile in SQL policy. A
configuration mismatch does not authorize rewriting that policy; canonical
keyword retrieval can remain available while mismatched semantic use is
unavailable. Keep model identity confirmation separate from policy changes.

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

Confirm the service version, complete `/tts` URL, selected voice, reference
audio and matching reference text/language, target language, and actual GPT and
SoVITS weights. `reference_voice` maps to the remote `ref_audio_path`, and
`style_prompt` maps to `prompt_text`. Record a trusted asset/version identity,
not just a display model name.

When the default voice has nonempty weight paths, Server initialization calls
the service's `set_gpt_weights` / `set_sovits_weights` endpoints. A failed switch
is logged and initialization continues with the service's loaded model. Before
starting that configuration, establish whether the service is a dedicated
validation instance and whether weight switching is within the agreed scope.
Initialization and health success do not prove the requested weights loaded.
Initial software validation can save synthesized audio privately without
playing it.

Easy-Turn, whisper.cpp and GPT-SoVITS adapters currently have no configurable
API-key or arbitrary authorization-header support. Confirm any gateway's
compatibility before choosing it. Easy-Turn's CLI listens only on loopback; its
adapter rejects URL credentials/query/fragment and disables proxy/redirect
following. Do not infer a supported remote authenticated deployment from a URL.

## Configuration and Acceptance Boundaries

Review the full effective configuration before any later model run:

1. The selected base file and recursively sorted same-name `.d` fragments are
   merged first. See [Server loading order](../../packages/alia-server/docs/configuration.md#loading-order).
2. `DATABASE_URL` and `ALIA_JWT_SECRET` override their fields. Provider API-key
   variables only fill empty keys in the matching default provider IDs
   `openai`, `anthropic` and `gemini`; they do not automatically fill custom IDs.
3. Persisted `settings.toml` overrides registered settings, with module-specific
   application timing. A provider can initialize from base configuration before
   later runtime settings apply, including TTS weight-switch side effects.
4. Committed SQL Context policy takes precedence over its old TOML projection;
   embedding profile admission remains enforced. Settings are not a universal
   replacement for startup configuration or policy authority.

Do not archive raw API keys, full settings responses or private settings files.
Record the configuration identity and credential source name/path without its
value. Unconfirmed deployment values stay marked pending in the release record.

Keep LLM `health_probe_enabled=false`. With a configured key, a disabled probe
reports Degraded; an empty key reports Unhealthy. This is not an inference test,
and the noop staging assertion that all modules are Healthy must not be copied
into a real-model acceptance. GPT-SoVITS health also accepts HTTP 400 from
`GET /tts`; it does not synthesize speech. Easy-Turn readiness/capabilities,
Whisper health and remote model names likewise need separate real-output and
loaded-model evidence. Preserve original request deadlines, concurrency and
cleanup ownership instead of changing them to obtain a passing result.

The two existing ignored targets `easy_turn_live` and `voice_stream_live` call
real Easy-Turn. The latter constructs its response/output in a fixture; neither
is a real LLM-to-TTS chain test. Reuse them only for their stated scope. The
[next-stage acceptance steps](../refactor/handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)
separate provider checks, Easy-Turn integration and the still-pending combined
model chain. No live target is enabled by this documentation update.

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

When Memory is enabled, startup accepts Healthy or Degraded and keeps it
enabled. Persistent Unhealthy after the configured attempts fails startup;
there is no silent policy change that disables Memory to make startup succeed.

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
