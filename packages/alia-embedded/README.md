# alia-embedded

Raspberry Pi 5 edge node for the Alia smart home assistant. Manages voice wake-word detection, Python subprocess lifecycle, plugin sandboxing, and device binding — all exposed over gRPC to the upper machine (`alia-server`).

## Prerequisites

- Rust toolchain (edition 2024)
- `protoc` (Protocol Buffers compiler)

## Project Structure

```
alia-embedded/
├── src/
│   ├── main.rs             # Entry point & gRPC server bootstrap
│   ├── config/             # TOML config types
│   ├── process/            # Python subprocess manager
│   ├── plugin/             # Plugin registry (install, manifest, sandbox)
│   ├── binding/            # Device binding state machine
│   ├── module/             # Module lifecycle & registry
│   └── grpc/               # tonic service implementations
```

## Quick Start

```sh
# From workspace root
cargo run -p alia-embedded
```

The binary looks for `config/embedded.toml` relative to the workspace root, but it no longer requires the file to exist on first boot.
If the file is missing, `alia-embedded` starts in provisioning mode, advertises discovery, generates a device keypair, and persists a runtime settings file automatically.

Runtime-owned state now lives separately from the settings file:

- `config/embedded.toml`: user-editable runtime settings
- `run/embedded-state.sqlite3`: SQLite-backed runtime state and metadata (binding status, direct-response rules)
- Disk directories such as `plugins/`, `run/*.pid`, key files, and future audio asset folders remain file-backed

Or override via environment variable:

```sh
export ALIA_EMBEDDED_CONFIG_PATH=config/embedded.toml
```

## Configuration

Only user-editable runtime settings live in `config/embedded.toml`. Core module defaults remain compiled into the binary.
Operational state that the system owns itself is stored in SQLite instead of being mixed into the config file.
The expected file shape is shown in [config/embedded.example.toml](../../config/embedded.example.toml), and the file can be created or updated by server/frontend OOBE through the embedded settings API.

| Section | Purpose |
|---------|---------|
| `[runtime]` | Provisioning state (`false` means accessory-style OOBE mode) |
| `[identity]` | Device ID, friendly name, keypair path |
| `[audio]` | Capture/playback devices, Opus encoding params, and Linux ALSA fallback names such as `alsa:plughw:0,0` |
| `[discovery]` | mDNS, BLE provisioning, bind security |

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
5. Open `run/embedded-state.sqlite3`
6. Create `BindingManager`, restore state from SQLite
7. Build & start gRPC server on `:50051`

## Architecture

See [docs/EDGE_DETAILS.md](../../docs/EDGE_DETAILS.md) for the full design — hardware spec, audio pipeline, plugin sandbox model, device binding protocol, and latency budgets.
