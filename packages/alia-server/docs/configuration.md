# Server Configuration

Server configuration is split into a base file plus semantic fragments. The
base file answers "which provider adapters and channels should run"; fragments
answer "how should each part run".

## Loading Order

The server resolves the base config from:

1. `ALIA_CONFIG_PATH`
2. `./config/server.toml`
3. user config directory
4. `/etc/alia/server.toml`

When the base file is `server.toml`, fragments under `server.d/**/*.toml` are
loaded recursively in sorted order and deep-merged.

## Setup Publication and Recovery

Setup validates the effective base plus fragments before publishing. It syncs
the candidate file, fragment files and directories, and the complete canonical parent
directory chain before reporting success. A private adjacent recovery record
identifies the original publication using paths and content hashes; it does not
contain configuration values or credentials. HTTP response fields and status
codes are unchanged: always check `success`, including for HTTP 200 responses.

Once publication has started, a sync failure means durability is unconfirmed.
The setup process retains the candidate, published files and recovery record.
Retry the same path, content and installation method to verify and finish that
publication. A different request cannot replace it. Cancellation exits setup
without deleting these files. On restart, select the same configuration path
(set `ALIA_CONFIG_PATH` for a custom path); the loader reconciles a pending
publication before reading it, even if the base was not yet published.

Recovery rejects changed or missing candidate/base/fragment content instead of
blindly overwriting it. Preserve the files and error details for operator review;
do not remove the recovery record merely to force a new setup. Normal loading
without a recovery record remains read-only and performs no durability sync.
Saving and recovering require access to sync the complete canonical ancestor
chain; unsupported filesystems or insufficient access fail explicitly without
changing permissions.
Configuration directories must remain under a trusted service account: this
protocol does not lock other local processes out of the file tree. A crash before
the recovery record exists can leave unowned temporary files or a CLI fragment
tree requiring manual review. Filesystem sync and injected-failure tests do not
establish physical power-loss guarantees for arbitrary storage hardware.

Runtime `settings.toml` writes are a separate path and are not covered by this
setup publication protocol.

## Recommended Layout

```text
config/
  server.toml
  server.d/
    providers/
      openai.toml
      anthropic.toml
      gemini.toml
      whisper_cpp.toml
      gpt_sovits.toml
    policies/
      routing.toml
      conversation.toml
      cache.toml
      memory.toml
      affect.toml
      regulation.toml
      expression.toml
      tools.toml
      automation.toml
    profiles/
      voice.default.toml
      expression_styles.toml
    resources/
      skills.toml
      cache_policy.toml
    plugins/
      ...
```

## Base Config Responsibilities

Keep these in `server.toml`:

- server bind address, port, and basic logging
- database connection and auth key material paths
- channel topology in `[[channels]]`
- non-LLM module selection in `[modules]`
- LLM role binding in `[llm.roles]`, including optional memory-capture and
  sentence-expression roles

Do not bury LLM role binding inside provider fragments. Provider fragments
configure external service connections; `profiles/llm.toml` chooses concrete
models and inference parameters.

Core server services are not configurable slots. The server always owns one
tool registry, skill registry, rule engine, scheduler, settings registry, sensor
registry, cache store, persona renderer, affect/expression runtimes, and
pipeline. Plugins extend these services through their registries.

## Fragment Responsibilities

Use fragments for implementation details:

- `providers/*.toml`: OpenAI-compatible, Anthropic, Gemini, whisper.cpp, GPT-SoVITS
- `profiles/llm.toml`: named chat and embedding model profiles
- `policies/*.toml`: routing, conversation, cache,
  memory, affect, regulation, expression, insights, rules, scheduler, tools
- `profiles/*.toml`: LLM profiles, voice profile, expression styles, and TTS reference
  parameters
- `resources/*.toml`: directories, runtime roots, model/service resource paths,
  and resource policy lists such as cache exclusion keywords
- `plugins/*.toml`: future plugin-owned configuration

Relative paths are resolved from the base config file directory.

## Voice Input

VoiceSession v2 is the sole embedded voice ingress. Configure
`[providers.easy_turn]` on the server for continuous turn inference; the
provider remains disabled until explicitly enabled. Embedded supplies ordered
mono 16 kHz capture and VAD evidence. The old utterance upload, local STT
correction role/policy, and channel interrupt flags have been removed.

Live voice cancellation is owned by the session token, and output Hold/Resume
uses the registered Playback v3 control contract. Existing duplex/companion
policies and shadow defaults are unchanged.

## Runtime Settings

`PATCH /api/v1/settings` updates frontend-visible settings. The response reports
which keys changed, which were applied live, which require restart, and which
were ignored by current runtime components.

The administrator-only update first commits any SQL-owned Context policy with
its version check, then updates desired settings, applies supported live changes,
and saves non-policy values to `settings.toml`. These are separate commit points:
`success: false` does not undo a policy commit or a module that already applied
its update. Inspect `context_policy_version`, `applied`, `module_reports`, and
`errors`; reload the current version before retrying a request with `If-Match`.
Explicitly requested keys are applied and saved again even when `changed` is
empty. A module apply failure does not prevent saving the desired settings.

For a non-policy update, the server rechecks that the caller is still an active
administrator after the update gate, policy read, and embedding previews, before
changing desired settings or applying modules. Demotion, deactivation, or deletion
observed at this admission point returns 403; a failed or timed-out authorization
query returns a redacted 503 after at most two seconds of query waiting. These
rejections do not apply or save the requested settings. The existing SQL policy
read may independently expire a timed pause. Policy and embedding changes retain
their transaction-local actor check and version comparison. Admission is not
continuous revocation: later demotion does not roll back an already admitted
update. Every explicit retry is a new request and must pass authorization again.

Settings saves serialize within the registry and publish a complete TOML
snapshot through a temporary file and same-directory atomic replacement.
Successful saves confirm file and parent-directory synchronization. A failure
before replacement leaves the previous file intact; a failure after replacement
reports that the new file is published but durability is unconfirmed. Live
changes remain in effect, and an explicit retry saves the current desired
snapshot. This runtime path has no setup-style recovery journal: restarting
after a failed save loads whichever complete old or new snapshot remains on
disk, while SQL continues to own Context policy. Ordinary loading stays read-only.

`PATCH /api/v1/memories/privacy` is a narrow administrator-only update of the
same global SQL policy. It supports the same quoted policy-version `If-Match`,
retains its existing response fields, and never writes a second file projection.
Authenticated privacy reads remain available and report the committed policy,
including changes made by another server instance.

Hot-updatable areas include many provider parameters and pipeline policies:

- LLM provider endpoints, model profile names, and timeouts
- STT/TTS endpoints and timeouts
- voice profile fields
- routing, affect, regulation, expression, cache, memory retrieval, and tool policies

Restart is expected for:

- server host/port boot behavior
- database connection settings
- auth secrets and server key material
- non-LLM adapter selection in `[modules]` and LLM role binding in `[llm.roles]`
- channel topology changes requiring adapter construction

## LAN Discovery

`alia-server` can advertise its control-plane endpoint on the local network with
mDNS so mobile clients can discover it before login.

Relevant `[server]` fields:

| Field | Purpose |
| --- | --- |
| `discovery_enabled` | Enables or disables server mDNS advertisement. |
| `discovery_service_name` | Defaults to `_alia-server._tcp`; keep it distinct from embedded nodes. |
| `discovery_instance_name` | Optional human-readable instance name shown by clients. |
| `public_endpoint` | Optional LAN-reachable URL to advertise when `host` is `0.0.0.0` or behind a proxy. |

Server discovery advertises TXT metadata with `kind=alia-server`,
`schema=alia.server.discovery.v1`, `mode=runtime` or `mode=oobe`, and an
`endpoint` URL. Embedded nodes continue to advertise separately as
`_alia-embedded._tcp`.

## Feature Flags

Default features enable the common runtime. Optional feature groups include:

| Feature | Purpose |
| --- | --- |
| `channel-embedded` | gRPC channel adapter for `alia-embedded`. |
| `llm-openai`, `llm-anthropic`, `llm-gemini` | LLM providers. |
| `stt-whisper`, `tts-sovits` | Speech provider adapters. |
| `embed-openai` | Embedding provider. |
| `tools-http` | Built-in HTTP/fetch tools. |
| `insights` | Background insights support. |
