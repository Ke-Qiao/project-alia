# alia-embedded

Raspberry Pi 5 edge node for the Alia smart home assistant. Manages voice wake-word detection, Python subprocess lifecycle, plugin sandboxing, and device binding — all exposed over gRPC to the upper machine (`alia-server`).

## Prerequisites

- Rust toolchain (edition 2024)
- `protoc` (Protocol Buffers compiler)

## Project Structure

```
alia-embedded/
├── config/
│   └── embedded.toml       # Runtime configuration
├── src/
│   ├── main.rs             # Entry point & gRPC server bootstrap
│   ├── config/             # TOML config types
│   ├── process/            # Python subprocess manager
│   ├── plugin/             # Plugin registry (install, manifest, sandbox)
│   ├── binding/            # Device binding state machine
│   └── grpc/               # tonic service implementations
├── run/                    # Runtime PID files & binding state (git-ignored)
└── plugins/                # Installed plugins (git-ignored)
```

## Quick Start

```sh
# From workspace root
cargo run -p alia-embedded
```

The binary expects `config/embedded.toml` relative to the workspace root.

Copy the example and edit:

```sh
cp config/embedded.example.toml config/embedded.toml
```

Run from the workspace root:

```sh
cargo run -p alia-embedded
```

Or override via environment variable:

```sh
export ALIA_EMBEDDED_CONFIG_PATH=config/embedded.toml
```

## Configuration

All settings live in a single `config/embedded.toml` at the workspace root. See `config/embedded.example.toml` for all fields.

| Section | Purpose |
|---------|---------|
| `[identity]` | Device ID, friendly name, keypair path |
| `[wakeword]` | Wake-word engine, model, keywords, sensitivity |
| `[audio]` | Capture/playback devices, Opus encoding params |
| `[discovery]` | mDNS, BLE provisioning, bind security |
| `[[process]]` | Managed Python subprocesses |
| `[[direct_response]]` | Keyword → local audio fast-path |

## Development

### Build

```sh
# Check only (fast)
cargo check -p alia-embedded

# Full build
cargo build -p alia-embedded
```

### Log Levels

Controlled via `RUST_LOG`:

```sh
RUST_LOG=debug cargo run -p alia-embedded    # verbose
RUST_LOG=alia_embedded=trace cargo run -p alia-embedded  # trace just this crate
```

### Proto Changes

Protobuf definitions live in `crates/proto/proto/`. After editing `.proto` files, a `cargo build` re-runs the build script automatically (via `tonic-build`).

```sh
# Rebuild protos + embedded
cargo build -p alia-proto -p alia-embedded
```

### gRPC Testing

The server listens on `0.0.0.0:50051`. Use `grpcurl` to interact:

```sh
# List services
grpcurl -plaintext localhost:50051 list

# List processes
grpcurl -plaintext localhost:50051 alia.process.ProcessManager/ListProcesses

# Start a process
grpcurl -plaintext -d '{"name":"mediapipe_face"}' \
  localhost:50051 alia.process.ProcessManager/StartProcess
```

### Adding a New gRPC Service

1. Define the service in a `.proto` file under `crates/proto/proto/`
2. Register it in `crates/proto/build.rs` and `crates/proto/src/lib.rs`
3. Implement the tonic trait in `src/grpc/`
4. Wire it into `main.rs` via `Server::builder().add_service(...)`

## Startup Sequence

1. Initialize tracing (env-filter)
2. Load `config/embedded.toml`
3. Create `ProcessManager`, clean orphan PIDs from `run/*.pid`
4. Create `PluginRegistry`, scan `plugins/` for manifests
5. Create `BindingManager`, restore state from `run/binding.json`
6. Build & start gRPC server on `:50051`

## Architecture

See [docs/EDGE_DETAILS.md](../../docs/EDGE_DETAILS.md) for the full design — hardware spec, audio pipeline, plugin sandbox model, device binding protocol, and latency budgets.
