# Configuration

Project Alia separates static startup configuration from runtime settings. The
goal is to make module selection explicit while allowing safe live updates for
provider parameters, policy tuning, voice profiles, and UI-managed settings.

## Server Config Layout

The server starts from a base config file:

- `ALIA_CONFIG_PATH`
- `./config/server.toml`
- user config directory
- `/etc/alia/server.toml`

For `server.toml`, fragments are loaded recursively from `server.d/**/*.toml`
and deep-merged in sorted path order. The examples use:

```text
config/
  server.example.toml
  server.example.d/
    providers/
    policies/
    profiles/
    resources/
```

The base file should own instance-level settings and module selection:

- server bind address and logging
- database and auth
- channel instances
- `[modules]` implementation choices

Fragments should own implementation details:

- `providers/*.toml`: OpenAI-compatible, Ollama, whisper.cpp, GPT-SoVITS, Mem0
- `policies/*.toml`: routing, conversation, cache, memory, insights, rules,
  scheduler, tools
- `profiles/*.toml`: voice profiles
- `resources/*.toml`: directories and resource roots
- future `plugins/*.toml`: plugin-owned configuration

Relative paths are resolved from the base config file directory, not from the
process working directory.

## Runtime Settings

The server settings registry exposes `core.*` and `plugin.*` keys through
`PATCH /api/v1/settings`. The response reports:

- `changed`: keys that changed in the registry
- `applied`: keys applied live by a module or pipeline policy
- `requires_restart`: keys that are intentionally startup-only
- `ignored_runtime`: keys that changed but no runtime component consumed
- `module_reports`: per-module apply results
- `errors`: runtime apply errors

Hot updates currently cover many provider parameters and pipeline policies,
including LLM endpoints/models, STT/TTS endpoints/timeouts, voice profile
fields, routing policy, cache policy, memory retrieval policy, and tool policy.

Restart is still expected for:

- server host/port and log-level boot behavior
- database connection settings
- auth secrets and key material
- module selection in `[modules]`
- channel topology changes that require adapter construction
- memory backend identity/API key changes that must pass startup health checks
- module slot changes such as switching the rule engine or scheduler backend

## Automation Config

The active server examples use:

```toml
[modules]
rule_engine = "automation"
scheduler = "automation"
```

The automation scheduler supports cron, one-shot, interval, condition-polling,
and event-subscription triggers. Rules and scheduler tasks share condition and
action registries, so plugin-defined conditions are available to both systems.
Sensor values are also available as a first-class condition dimension.

## Embedded Config

`alia-embedded` can boot with compiled defaults when `config/embedded.toml` is
missing. The example file covers:

- identity and binding state
- wake word model and keywords
- audio capture/playback device, backend, buffering, and volume defaults
- audio debug WAV recording
- edge STT timing and language
- VAD model and endpointing thresholds
- playback interruption behavior
- continuous dialogue windows
- direct response audio directory
- mDNS/BLE discovery
- local development binding bypass

Volume settings and direct response metadata are also persisted in embedded
SQLite so the frontend/server can update them without rewriting the whole config
file.

## Embedded Settings gRPC

The embedded settings service supports:

- get/update settings snapshot
- get/set microphone and speaker volume
- list/upsert/delete direct response rules

Settings responses include `requires_restart` where the device cannot apply a
change safely at runtime.
