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
2. Start external model services you selected in server config.
3. Set embedded local-dev binding fields in `config/embedded.toml` only for
   development.
4. Run `alia-server`.
5. Run `alia-embedded`.
6. Watch server debug output and embedded logs while triggering the wake word.

If you do not need the frontend, the CLI channel and embedded channel are enough
to validate the core runtime.

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
