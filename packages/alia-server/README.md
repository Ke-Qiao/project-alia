# alia-server

The central server for the Alia personal AI assistant system. Handles API requests, WebSocket connections, AI pipeline orchestration, and communication with `alia-embedded` via gRPC.

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

### 3. Build & Run

```bash
# From workspace root
cargo run -p alia-server

# Or with specific features
cargo run -p alia-server --features "llm-openai,stt-whisper,tts-sovits"
```

### 4. Check It's Running

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
├── api/               # HTTP route handlers (REST endpoints)
├── auth/              # JWT, password hashing, auth middleware
├── embedded/          # gRPC client for alia-embedded
├── module/            # Module lifecycle, registry, builder, factory
├── providers/         # AI adapter trait definitions + noop implementations
│   ├── adapter.rs     # All trait definitions (LlmAdapter, SttAdapter, etc.)
│   ├── llm/           # LLM, STT, TTS noop implementations
│   ├── embedding/     # Embedding adapter
│   ├── memory/        # Memory backend (GraphRAG)
│   ├── cache/         # Cache backend (two-layer)
│   ├── skills/        # Skill provider
│   ├── persona/       # Persona/prompt provider
│   ├── meditation/    # Background meditation strategy
│   └── router/        # Intent routing strategy
├── rules/             # Rule engine
├── ws/                # WebSocket handler
└── openapi.rs         # OpenAPI (utoipa) doc generation
```

## Architecture

The server uses a **module-slot architecture** — all subsystems (LLM, STT, TTS, memory, etc.) are defined as `dyn Trait` objects, loaded at startup based on configuration. This enables:

- Swapping implementations without code changes (e.g., OpenAI → Ollama)
- Feature-gated compilation (only include what you need)
- Clean separation between interface and implementation

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
