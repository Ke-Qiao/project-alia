# Server Configuration

Server configuration is split into a base file plus module-specific fragments.
The base file answers "what should run"; fragments answer "how should each part
run".

## Loading Order

The server resolves the base config from:

1. `ALIA_CONFIG_PATH`
2. `./config/server.toml`
3. user config directory
4. `/etc/alia/server.toml`

When the base file is `server.toml`, fragments under `server.d/**/*.toml` are
loaded recursively in sorted order and deep-merged.

## Recommended Layout

```text
config/
  server.toml
  server.d/
    providers/
      openai.toml
      ollama.toml
      whisper_cpp.toml
      gpt_sovits.toml
      mem0.toml
    policies/
      routing.toml
      conversation.toml
      cache.toml
      memory.toml
      tools.toml
      automation.toml
    profiles/
      voice.default.toml
    resources/
      paths.toml
    plugins/
      ...
```

## Base Config Responsibilities

Keep these in `server.toml`:

- server bind address, port, and basic logging
- database connection and auth key material paths
- channel topology in `[[channels]]`
- `[modules]` slot selection

Do not bury module selection inside provider fragments. If a frontend changes a
provider setting at runtime, it should not accidentally change which module is
active.

## Fragment Responsibilities

Use fragments for implementation details:

- `providers/*.toml`: OpenAI-compatible, Ollama, whisper.cpp, GPT-SoVITS, Mem0
- `policies/*.toml`: routing, conversation, cache, memory, insights, rules,
  scheduler, tools
- `profiles/*.toml`: voice profile and TTS reference parameters
- `resources/*.toml`: directories, runtime roots, model/service resource paths
- `plugins/*.toml`: future plugin-owned configuration

Relative paths are resolved from the base config file directory.

## Runtime Settings

`PATCH /api/v1/settings` updates frontend-visible settings. The response reports
which keys changed, which were applied live, which require restart, and which
were ignored by current runtime components.

Hot-updatable areas include many provider parameters and pipeline policies:

- LLM endpoints, model names, and timeouts
- STT/TTS endpoints and timeouts
- voice profile fields
- routing, cache, memory retrieval, and tool policies

Restart is expected for:

- server host/port boot behavior
- database connection settings
- auth secrets and server key material
- module selection in `[modules]`
- channel topology changes requiring adapter construction
- memory backend identity/API key changes that must pass startup health checks

## Feature Flags

Default features enable the common runtime. Optional feature groups include:

| Feature | Purpose |
| --- | --- |
| `channel-embedded` | gRPC channel adapter for `alia-embedded`. |
| `llm-openai`, `llm-ollama` | LLM providers. |
| `stt-whisper`, `tts-sovits` | Speech provider adapters. |
| `embed-ollama`, `embed-openai` | Embedding providers. |
| `memory-mem0` | Mem0 memory backend. |
| `cache-semantic` | Semantic cache support. |
| `tools-http` | Built-in HTTP/fetch tools. |
| `insights` | Background insights support. |

