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
# Edit config/server.toml with your settings (API keys, DB URL, etc.)
```

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
├── embedded/          # Legacy gRPC client (superseded by channels/embedded.rs)
├── module/            # Module lifecycle, registry, builder, factory
├── providers/         # AI adapter trait definitions + implementations
│   ├── adapter.rs     # All trait definitions (LlmAdapter, SttAdapter, etc.)
│   ├── llm/           # LLM adapters (OpenAI, Ollama)
│   ├── stt/           # Speech-to-text (whisper.cpp)
│   ├── tts/           # Text-to-speech (GPT-SoVITS)
│   ├── embedding/     # Embedding adapters (Ollama, OpenAI)
│   ├── memory/        # Memory backend (GraphRAG + SimpleMemory)
│   ├── cache/         # Cache backend (two-layer: exact + semantic)
│   ├── skills/        # Skill provider (builtin skills engine)
│   ├── persona/       # Persona/prompt provider (Handlebars templates)
│   ├── meditation/    # Background meditation strategy
│   └── router/        # Intent routing (MultiLayerRouter + Passthrough)
├── rules/             # Rule engine
├── scheduler/         # Cron/interval task scheduler
├── tools/             # Built-in tools + plugin tool registration
└── ws/                # WebSocket handler + subscription/backpressure
```

## Architecture

The server uses two complementary patterns:

- **Module-slot architecture** — Single-instance subsystems (LLM, STT, TTS, memory, etc.) are defined as `dyn Trait` objects in a `ModuleRegistry`, loaded at startup based on `[modules]` configuration. Swapping implementations (e.g., OpenAI → Ollama) requires only a config change.

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

| Flag | Description |
|------|-------------|
| `llm-openai` | OpenAI ChatGPT adapter |
| `llm-ollama` | Ollama local LLM adapter |
| `stt-whisper` | whisper.cpp STT adapter |
| `tts-sovits` | GPT-SoVITS TTS adapter |
| `embed-ollama` | Ollama embedding adapter |
| `memory-graphrag` | GraphRAG memory backend |
| `cache-semantic` | Two-layer semantic cache |
| `meditation` | Background meditation/reflection |
