# alia-embedded

Raspberry Pi 5 edge node for the Alia smart home assistant. Manages voice wake-word detection, local audio, native expression/motion, YuNet face tracking, plugin sandboxing, sensors, and device binding — all exposed over gRPC to the upper machine (`alia-server`).

## Prerequisites

- Rust toolchain (edition 2024)
- `protoc` (Protocol Buffers compiler)
- OpenCV development libraries when building with `--features vision-opencv`

## Project Structure

```
alia-embedded/
├── src/
│   ├── main.rs             # Entry point & gRPC server bootstrap
│   ├── config/             # TOML config types
│   ├── plugin/             # Plugin registry (install, manifest, sandbox)
│   ├── binding/            # Device binding state machine
│   ├── module/             # Module lifecycle & registry
│   └── grpc/               # tonic service implementations
```

## Documentation

Package docs live in [docs/](docs/README.md). Start there for the current edge
runtime architecture, configuration, audio/voice flow, gRPC services, and sensor
provider model. For Raspberry Pi YuNet/OpenCV setup, see
[docs/vision-yunet.md](docs/vision-yunet.md).

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
- Disk directories such as `plugins/`, key files, and future audio asset folders remain file-backed

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
| `[audio]` | Capture/playback backend selection, device names, Opus params, and Linux ALSA names such as `alsa:plughw:0,0` |
| `[motion]` | Neck matrix/animation assets, PCA9685/I2C settings, and face-center to rotation tuning |
| `[vision]` | Camera and OpenCV YuNet face tracker settings |
| `[discovery]` | mDNS, BLE provisioning, bind security |

## Development

### Build

```sh
# Check only (fast)
cargo check -p alia-embedded

# Check YuNet/OpenCV vision on a machine with OpenCV installed
cargo check -p alia-embedded --features vision-opencv

# Prepare Pi 5 OpenCV/YuNet dependencies and model
./scripts/setup-embedded-vision-pi.sh

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
```

### Adding a New gRPC Service

1. Define the service in a `.proto` file under `crates/proto/proto/`
2. Register it in `crates/proto/build.rs` and `crates/proto/src/lib.rs`
3. Implement the tonic trait in `src/grpc/`
4. Wire it into `main.rs` via `Server::builder().add_service(...)`

## Startup Sequence

1. Initialize tracing (env-filter)
2. Load `config/embedded.toml`
3. Create `PluginRegistry`, scan `plugins/` for manifests
4. Open `run/embedded-state.sqlite3`
5. Create `BindingManager`, restore state from SQLite
6. Build & start gRPC server on `:50051`

## Architecture

See [docs/architecture.md](docs/architecture.md) for the current embedded
architecture and [docs/audio-and-voice.md](docs/audio-and-voice.md) for the
voice pipeline details.
