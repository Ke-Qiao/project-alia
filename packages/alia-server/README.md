# alia-server

The central server for the Alia personal AI assistant system. Handles API requests, WebSocket connections, AI pipeline orchestration, and multi-channel communication (gRPC to `alia-embedded`, interactive CLI REPL, and more).

## Prerequisites

- Rust 1.85+ (2024 edition)
- PostgreSQL 15+
- A running `alia-embedded` instance (optional, for device control)

## Getting Started

### 1. Database Setup

```bash
# Start PostgreSQL via Docker (from project root)
docker compose up -d postgres
```

### 2. Configuration

Config files live in the workspace root `config/` directory.

Copy the example and edit:

```bash
# From workspace root
cp config/server.example.toml config/server.toml
cp -R config/server.example.d config/server.d
# Edit config/server.toml and semantic fragments under config/server.d/.
```

`server.toml` is the base config and owns `[modules]` adapter selection plus
`[llm.roles]` model-profile binding. Files
under `server.d/**/*.toml` are loaded recursively in sorted order and
deep-merged. Keep external service settings in `server.d/providers/`, behavior
settings in `server.d/policies/`, LLM/voice/profile settings in
`server.d/profiles/`, and future plugin settings in `server.d/plugins/`.

The `mise.toml` at the workspace root automatically sets `ALIA_CONFIG_PATH=config/server.toml`.
To override manually:

```bash
export ALIA_CONFIG_PATH=config/server.toml
```

Individual values can also be overridden via environment variables:

- `DATABASE_URL` — PostgreSQL connection string
- `JWT_SECRET` — Secret key for JWT signing
- `OPENAI_API_KEY` — OpenAI API key (if using OpenAI adapter)

### 3. Channel Configuration

Channels are configured as an array in `server.toml`. Each entry creates a channel adapter instance:

```toml
# gRPC channel to a Raspberry Pi
[[channels]]
type = "embedded"
id = "pi-living-room"
enabled = true
endpoint = "http://192.168.1.100:50051"

# Interactive CLI for development
[[channels]]
type = "cli"
id = "dev-cli"
enabled = true
```

### 4. Build & Run

```bash
# From workspace root
cargo run -p alia-server

# Or with specific features
cargo run -p alia-server --features "llm-openai,stt-whisper,tts-sovits"
```

### 5. Check It's Running

```bash
curl http://localhost:3000/health
# → {"status":"ok"}
```

API documentation is available at `http://localhost:3000/scalar`.

## Documentation

Package docs live in [docs/](docs/README.md). Start there for the current
architecture, configuration, channels, sensors, runtime systems, and extension
points.

## Project Structure

```
src/
├── main.rs            # Entry point, startup sequence
├── config.rs          # TOML config loading + env overrides
├── error.rs           # Unified error type (AppError → HTTP responses)
├── event.rs           # Internal event bus (broadcast channel)
├── state.rs           # Shared application state (AppState)
├── openapi.rs         # OpenAPI (utoipa) doc generation
├── api/               # HTTP route handlers (REST endpoints)
├── auth/              # JWT, password hashing, auth middleware
├── channels/          # Channel adapter system
│   ├── adapter.rs     # ChannelAdapter trait + types (Event, Response, Capabilities)
│   ├── registry.rs    # ChannelRegistry (multi-instance, broadcast, health)
│   ├── embedded.rs    # EmbeddedChannel (gRPC → alia-embedded, lazy connect + retry)
│   └── cli.rs         # CliChannel (rustyline REPL, history, colored output)
├── embedded/          # Reusable embedded gRPC client helpers
├── module/            # Module lifecycle, registry, builder, factory
├── providers/         # AI adapter trait definitions + implementations
│   ├── adapter.rs     # All trait definitions (LlmAdapter, SttAdapter, etc.)
│   ├── llm/           # LLM adapters (OpenAI, Anthropic, Gemini)
│   ├── stt/           # Speech-to-text (whisper.cpp)
│   ├── tts/           # Text-to-speech (GPT-SoVITS)
│   ├── embedding/     # Embedding adapters (OpenAI)
│   ├── memory/        # Built-in Alia memory system
│   ├── cache/         # Cache backend (two-layer: exact + semantic)
│   ├── skills/        # Skill provider (builtin skills engine)
│   ├── persona/       # Persona/prompt provider (Handlebars templates)
│   ├── insights/      # Background insights strategy
│   └── router/        # Intent routing (MultiLayerRouter + Passthrough)
├── rules/             # Rule engine
├── scheduler/         # Cron/interval task scheduler
├── tools/             # Built-in tools + plugin tool registration
└── ws/                # WebSocket handler + subscription/backpressure
```

## Architecture

The server uses two complementary patterns:

- **Fixed core + provider adapters** — The pipeline, tool registry, skill registry, scheduler, rule engine, settings, sensors, persona rendering, cache store, and dependency graph are fixed core services. `[modules]` selects edge providers such as LLM, STT, TTS, embeddings, and memory; plugins extend core services through registration APIs instead of replacing them.

- **Channel adapter architecture** — Multi-instance I/O channels (gRPC to embedded devices, CLI REPL, future: Telegram/Discord/Web) are registered in a `ChannelRegistry`. Each channel implements the `ChannelAdapter` trait and declares its capabilities (text, voice, streaming, sensors, etc.). Channels are configured as a `[[channels]]` array, allowing multiple instances of the same type.

## Development

```bash
# Check compilation
cargo check -p alia-server

# Run tests
cargo test -p alia-server

# Check with all features
cargo check -p alia-server --all-features
```

## Feature Flags

All features are enabled by default. Disable with `--no-default-features` and selectively enable what you need.

| Flag | Description | Pulls in |
|------|-------------|----------|
| `channel-embedded` | gRPC channel adapter for alia-embedded devices | `alia-proto`, `tonic` |
| `llm-openai` | OpenAI ChatGPT adapter | `reqwest` |
| `llm-anthropic` | Anthropic Messages API adapter | `reqwest` |
| `llm-gemini` | Google Gemini API adapter | `reqwest` |
| `stt-whisper` | whisper.cpp STT adapter | `reqwest` |
| `tts-sovits` | GPT-SoVITS TTS adapter | `reqwest` |
| `embed-openai` | OpenAI embedding adapter | `reqwest` |
| `insights` | Background insights/reflection | — |

```bash
# Minimal build (no AI backends, no embedded channel)
cargo build -p alia-server --no-default-features

# Only local AI + voice
cargo build -p alia-server --no-default-features --features "llm-gemini,stt-whisper,tts-sovits"

# Full build (default)
cargo build -p alia-server
```
