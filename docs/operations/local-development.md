# Local Development

This guide covers Server and Embedded development. Web, iOS and Android work is deferred; Node/pnpm and a frontend are not required for this workflow. Provider-specific setup is documented in [model services](model-services.md).

## Prerequisites

- `mise` for toolchain versions
- Docker for PostgreSQL
- Rust and Cargo through the workspace toolchain
- `protoc` for protobuf builds if your environment does not already provide it
- `jq` for the optional HTTP setup examples

## First Run

Run these commands from the repository root for a fresh checkout without
`config/server.toml`, `config/server.d` or `config/embedded.toml`. Reuse existing
configuration instead of copying defaults over it.

```bash
mise install rust
docker compose up -d postgres
cp config/server.example.toml config/server.toml
cp -R config/server.example.d config/server.d
cp config/embedded.example.toml config/embedded.toml
```

The workspace `mise.toml` also lists Node and pnpm for deferred frontend work;
plain `mise install` installs those too, so the command above selects Rust only.
Use an activated mise shell or its Rust shims for the Cargo commands below.
The supplied Compose service publishes PostgreSQL on host port 5432, matching
the Server example. If using an existing database at another address, set
`[database].url` or the overriding `DATABASE_URL` accordingly.

Edit `config/server.toml` to choose modules and channels. Then tune providers in
`config/server.d/providers/` and policies in `config/server.d/policies/`.

As an alternative to copying Server files, run Server with an absent
`ALIA_CONFIG_PATH` target and use its CLI or local HTTP setup; follow the exact
[generate/edit/save flow](server-embedded.md#配置与启动). CLI generation creates
the base plus `.d` layout and refuses existing targets. HTTP generation returns
a complete single-file template, so edit that file directly unless you choose
to add overrides in its same-name `.d` directory. Existing fragments retain
normal precedence. Neither setup path tests database connectivity, model
availability or hardware readiness.

Detailed package setup lives in:

- [alia-server configuration](../../packages/alia-server/docs/configuration.md)
- [alia-embedded configuration](../../packages/alia-embedded/docs/configuration.md)

## Run Components

```bash
cargo run -p alia-server
cargo run -p alia-embedded
```

The repo also includes convenience tasks through `mise` and `package.json`; use
those if they match your local workflow.

`ALIA_CONFIG_PATH` selects the Server file; this repository's `mise.toml` sets it
to `config/server.toml`. `ALIA_EMBEDDED_CONFIG_PATH` independently selects the
Embedded file. Existing invalid Server configuration causes startup to fail;
OOBE only starts when configuration is absent. Its listener is always
`127.0.0.1:3000` until setup completes, after which the configured Server port
applies.

## Same-Machine Voice Test

For local server + embedded testing:

1. Start PostgreSQL.
2. Start external model services you selected in server config. For device
   VoiceSession, start Easy-Turn on the Server host, including when Embedded runs
   on a Pi 5; see [model services](./model-services.md).
3. Choose the normal device binding flow or, only on a trusted development
   machine, the Embedded `[dev]` binding fields in `config/embedded.toml`.
4. Run `alia-server`.
5. Run `alia-embedded`.
6. Speak the wake word and watch the authenticated Ready, continuous capture,
   and server turn decisions. Capture remains active during native playback.

Before enabling voice input, register an active Server account through the
[REST account flow](server-embedded.md#账号与现有管理入口), set the Embedded
channel's `owner_user_id` to that account's UUID in Server configuration, and
restart Server after this file change. Embedded development binding does not
assign an account owner or bypass Server voice authorization.

For continuous voice, enable `[providers.easy_turn]` in the Server provider
fragment and `[speech].enabled` in Embedded. The device has one continuous
VoiceSession path; capture must be 16 kHz mono. Build and deploy Server and
Embedded together: voice v2
requires software hold capability and playback v3; old versions are rejected.
After reconnecting or rebinding, require a fresh wake and a new voice session.
An optional wake cue starts only after Ready and uses the same scoped native
mixer as conversation audio.

Add a Server policy fragment such as `config/server.d/policies/duplex.toml` when
an explicit hold budget is needed:

```toml
[policies.duplex]
hold_budget_ms = 60000
```

The default is 60 seconds, with an integer range of 1–120000 milliseconds and
further limits from the peer and original output lifetime. A suspected
interruption first creates a temporary hold with a 3-second semantic deadline;
a fresh Backchannel may resume it. Explicit WAIT retains the original answer
and requires an explicit Continue. Repeated WAIT and Resume do not renew the
original deadlines. Stop, a new question, expiry or source revocation retires
the old output. These times are engineering defaults, not measured acoustic
latency targets; see the [voice protocol](../refactor/voice-protocol.md).

Use the CLI and Embedded channels for this voice workflow. Software hold/control receipts and model labels do not establish
physical silence, AEC quality or Pi speaker timing. Validate the real audio
path separately before treating a local software run as a hardware result.

## Useful Diagnostics

- Enable `server.debug_output` for high-level router, LLM, tool, STT, TTS, and
  channel traces.
- Enable embedded `audio_debug` only while diagnosing audio; WAV files can grow
  quickly.
- Use `/health` for aggregated module health. A responding HTTP endpoint is not proof that every provider is ready; inspect `status` and each module.
- Aggregation currently has no overall deadline, profile deduplication or request concurrency limit; the router may probe the minor LLM again. Keep live probes disabled for routine polling until an explicit diagnostic budget is chosen. The existing 5-second OpenAI probe limit applies to one probe, not the whole endpoint.
- Use authenticated `/api/v1/settings` to inspect runtime settings; durable policy changes require the documented revision and administrator permissions.
- Check embedded gRPC settings for volume and direct response state.

## Common Local Paths

- Server config: `config/server.toml`
- Server fragments: `config/server.d/`
- Embedded config: `config/embedded.toml`
- Direct response example bundles: `config/direct_response.example/`
- Embedded direct response runtime directory: `run/direct-responses/`
- Embedded audio debug directory: `run/audio-debug/`


## Administration without a frontend

See [Server/Embedded operations](server-embedded.md) for the supported setup,
binding, policy, memory, execution and diagnostic entry points. Companion defaults
to shadow; [its REST guide](companion.md) explains explicit opt-in, revision
conflicts and uncertain completion. After deployment, test the actual device;
local software pairing does not establish microphone, AEC or speaker quality.
