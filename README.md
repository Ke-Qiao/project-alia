# Project Alia

A personal AI assistant system with voice interaction, smart home control, and an extensible plugin architecture. Runs as a split-brain setup: a server (upper machine) handles AI orchestration, and a Raspberry Pi edge node (lower machine) manages hardware I/O.

```mermaid
graph LR
    Frontend["alia-frontend<br/>(Expo, RN)"] <-->|"REST + WebSocket<br/>port 3000"| Server["alia-server<br/>(Axum, Rust)"]
    Server <-->|"gRPC (HTTP/2)<br/>port 50051"| Embedded["alia-embedded<br/>(Tonic, RPi)"]
    Server --- DB[("PostgreSQL<br/>(SeaORM)")]
    Server -.-|"CLI REPL<br/>(rustyline)"| CLI["alia> prompt"]
    Embedded --- Proc["tokio::process<br/>(Python subprocesses)"]
```

## Packages

| Package | Description |
|---------|-------------|
| [alia-server](packages/alia-server/) | Central server — API, AI pipeline, WebSocket, channel adapters, module orchestration |
| [alia-embedded](packages/alia-embedded/) | Raspberry Pi edge node — wake-word, process management, plugin sandbox, gRPC |
| [alia-frontend](packages/alia-frontend/) | Cross-platform consumer app (React Native / Expo) |

### Shared Crates

| Crate | Description |
|-------|-------------|
| [proto](crates/proto/) | Shared Protobuf definitions and generated code |
| [transport](crates/transport/) | USB/WiFi transport layer abstraction |

## Prerequisites

- [mise](https://mise.jdx.dev/) (manages Node, pnpm, Rust versions automatically)
- Docker (for PostgreSQL)
- `protoc` (Protocol Buffers compiler)

## Quick Start

```bash
# 1. Install tool versions via mise
mise install

# 2. Start PostgreSQL
docker compose up -d

# 3. Copy and edit config files
cp config/server.example.toml config/server.toml
cp -R config/server.example.d config/server.d
cp config/embedded.example.toml config/embedded.toml

# 4. Run the server
cargo run -p alia-server

# 5. Run the embedded node (on Raspberry Pi or locally for dev)
cargo run -p alia-embedded

# 6. Run the frontend
cd packages/alia-frontend && pnpm install && pnpm dev
```

Or use the convenience scripts:

```bash
# Run everything at once (DB + server + embedded + frontend)
pnpm dev

# Backend only (DB + server + embedded)
pnpm dev:backend
```

## Mise Tasks

```bash
mise run dev-server     # cargo run -p alia-server
mise run dev-frontend   # pnpm dev (in alia-frontend)
mise run check          # cargo check --workspace
mise run db             # docker compose up -d postgres
```

## Project Structure

```
project-alia/
├── config/                   # Configuration files (gitignored, examples tracked)
│   ├── server.example.toml
│   └── embedded.example.toml
├── crates/
│   ├── proto/                # Shared Protobuf definitions
│   └── transport/            # USB/WiFi transport abstraction
├── packages/
│   ├── alia-server/          # Server (Rust / Axum)
│   ├── alia-embedded/        # Edge node (Rust / Tonic)
│   └── alia-frontend/        # Frontend (React Native / Expo)
├── docs/                     # Architecture & design documents
├── Cargo.toml                # Rust workspace root
├── package.json              # pnpm workspace root
├── mise.toml                 # Tool versions & dev tasks
└── docker-compose.yml        # PostgreSQL
```

## Tech Stack

- **Server**: Rust, Axum, SeaORM, tokio, utoipa, rustyline
- **Embedded**: Rust, Tonic (gRPC), tokio::process
- **Frontend**: TypeScript, React Native, Expo
- **Communication**: REST + WebSocket (frontend ↔ server), gRPC (server ↔ embedded), CLI REPL (local dev)
- **Database**: PostgreSQL 17
- **Tooling**: mise, pnpm, Docker

## Feature Flags (alia-server)

The server uses Cargo feature flags for conditional compilation. All features are enabled by default.

| Feature | Description |
|---------|-------------|
| `channel-embedded` | gRPC channel adapter for communicating with alia-embedded |
| `llm-openai` | OpenAI GPT backend |
| `llm-ollama` | Local Ollama LLM backend + multi-layer intent router |
| `stt-whisper` | Whisper speech-to-text |
| `tts-sovits` | SoVITS text-to-speech |
| `embed-ollama` | Ollama embedding backend |
| `embed-openai` | OpenAI embedding backend |
| `memory-graphrag` | GraphRAG memory system |
| `cache-semantic` | Two-layer semantic cache |
| `meditation` | Scheduled meditation system |

Build a minimal server (no AI backends, no embedded channel):

```bash
cargo build -p alia-server --no-default-features
```

Build with only specific features:

```bash
cargo build -p alia-server --no-default-features --features "llm-ollama,stt-whisper"
```

## License

[MIT](LICENSE)
