# Local Development

This is the short path for running Project Alia on one development machine. It
is intentionally high level; provider-specific setup belongs in model/provider
docs or scripts.

## Prerequisites

- `mise` for toolchain versions
- Docker for PostgreSQL
- Rust and Cargo through the workspace toolchain
- Node/pnpm for the frontend
- `protoc` for protobuf builds if your environment does not already provide it

## First Run

```bash
mise install
docker compose up -d
cp config/server.example.toml config/server.toml
cp -R config/server.example.d config/server.d
cp config/embedded.example.toml config/embedded.toml
```

Edit `config/server.toml` to choose modules and channels. Then tune providers in
`config/server.d/providers/` and policies in `config/server.d/policies/`.

Detailed package setup lives in:

- [alia-server configuration](../../packages/alia-server/docs/configuration.md)
- [alia-embedded configuration](../../packages/alia-embedded/docs/configuration.md)

## Run Components

```bash
cargo run -p alia-server
cargo run -p alia-embedded
cd packages/alia-frontend
pnpm install
pnpm dev
```

The repo also includes convenience tasks through `mise` and `package.json`; use
those if they match your local workflow.

## Same-Machine Voice Test

For local server + embedded testing:

1. Start PostgreSQL.
2. Start external model services you selected in server config. For continuous
   `server_turn`, start Easy-Turn on the Server host, including when Embedded runs
   on a Pi 5; see [model services](./model-services.md).
3. Set embedded local-dev binding fields in `config/embedded.toml` only for
   development.
4. Run `alia-server`.
5. Run `alia-embedded`.
6. Watch server debug output and embedded logs while triggering the wake word.

For continuous voice, enable `[providers.easy_turn]` in the Server provider
fragment and select `mode = "server_turn"` under Embedded `[speech]`. Capture
must be 16 kHz mono. Build and deploy Server and Embedded together: voice v2
requires software hold capability and playback v3; old versions are rejected.
After reconnecting or rebinding, obtain a new voice session instead of reusing
a retired session identity.

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

Use the CLI and Embedded channels for this voice workflow; the frontend is
optional. Software hold/control receipts and model labels do not establish
physical silence, AEC quality or Pi speaker timing. Validate the real audio
path separately before treating a local software run as a hardware result.

## Useful Diagnostics

- Enable `server.debug_output` for high-level router, LLM, tool, STT, TTS, and
  channel traces.
- Enable embedded `audio_debug` only while diagnosing audio; WAV files can grow
  quickly.
- Use `/health` for basic server liveness.
- Use `/api/v1/settings` to inspect frontend-visible runtime settings.
- Check embedded gRPC settings for volume and direct response state.

## Common Local Paths

- Server config: `config/server.toml`
- Server fragments: `config/server.d/`
- Embedded config: `config/embedded.toml`
- Direct response example bundles: `config/direct_response.example/`
- Embedded direct response runtime directory: `run/direct-responses/`
- Embedded audio debug directory: `run/audio-debug/`
