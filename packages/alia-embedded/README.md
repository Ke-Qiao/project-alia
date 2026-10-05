# alia-embedded

Raspberry Pi 5 edge node for the Alia smart home assistant. The library runtime owns continuous VoiceSession input, wake word and VAD evidence, scoped native playback, local cues, signed BodyControl, optional YuNet face tracking, sensors, plugins, and device binding. Easy-Turn and response generation run in `alia-server`.

## Prerequisites

- Rust toolchain (edition 2024)
- `protoc` (Protocol Buffers compiler)
- OpenCV development libraries when building with `--features vision-opencv`

## Project Structure

```
alia-embedded/
├── src/
│   ├── main.rs             # CLI host for the library runtime
│   ├── lib.rs              # Instance-local runtime API
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
If the file is missing, compiled defaults select provisioning mode. Selected modules still require their build features, models, and hardware; use explicit `noop` slots for service-only development.

Runtime-owned state now lives separately from the settings file:

- `config/embedded.toml`: user-editable runtime settings
- `[device_runtime].state_path`: SQLite state and metadata; default `run/embedded-state.sqlite3`, resolved relative to the config directory
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
| `[device_runtime]` | Bind address, instance-local state paths, and cleanup budgets |
| `[modules]` | Selected backends; `noop` explicitly disables a slot |
| `[audio]` | Capture/playback devices, mono 16 kHz capture, fixed native output format, and bounded buffers |
| `[speech]` | Exact Server channel instance ID, continuous VoiceSession pre-roll, and upload queue |
| `[motion]` | Neck matrix/animation assets, PCA9685/I2C settings, and face-center to rotation tuning |
| `[vision]` | Camera and OpenCV YuNet face tracker settings |
| `[discovery]` | mDNS, BLE provisioning, bind security |

### Raspberry Pi Bluetooth / BlueZ

BLE provisioning is optional and separate from mDNS. Keep
`modules.discovery = "mdns"` for normal operation; `ble_provision_enabled = true`
adds the nearby phone beacon plus a GATT Wi-Fi provisioning service. The phone
app writes JSON credentials to characteristic
`3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f3` and reads status from
`3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f2`. The embedded process applies Wi-Fi via
NetworkManager `nmcli`, so credentials are not stored in `embedded.toml`.
The BLE advertisement intentionally stays small and only advertises the Alia
service UUID; full device metadata remains in mDNS and GATT because many Pi
controllers still reject legacy advertising payloads above 31 bytes.

On a Pi that cannot start BlueZ or advertise BLE, verify the OS layer first:

```sh
sudo apt update
sudo apt install -y bluez rfkill
sudo systemctl enable --now bluetooth
systemctl status bluetooth --no-pager
rfkill list bluetooth
sudo rfkill unblock bluetooth
bluetoothctl show
```

`bluetoothctl show` should list a controller with `Powered: yes`. `nmcli` should
also be present for Wi-Fi provisioning. If BLE is not needed during bring-up,
set `[discovery].ble_provision_enabled = false`; mDNS discovery via
`_alia-embedded._tcp` will continue to work.

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
4. Wire it into the library runtime builder and update contract/lifecycle tests

## Voice and Runtime Lifecycle

`EmbeddedBuilder` constructs the runtime with instance-local resources and a
cleanup owner. Capture starts before the speech worker; a Noop capture host
keeps speech inactive with degraded health. A real capture backend requires an
available VAD. Shutdown cancels and joins voice/network/cue work before resource
cleanup completes.

After wake word, authenticated VoiceSession v2 uploads continuously, including
during playback. Local cues wait for Ready and retain its original scope.
Native output mixes conversation at gain 1.0 and cues at 0.2. Hold suppresses cue
tails; scoped retirement cancels both sources. Playback v3 reports native
submission evidence without claiming physical presentation or AEC validation.

The old utterance RPCs, edge STT path, thinking/follow-up timers, configurable
mixer slot, and unscoped direct-response/stop controls have been removed. An
explicit input-session close command is deferred; output Fence does not serve
as an acknowledged microphone-session close.

## Architecture

See [docs/architecture.md](docs/architecture.md) for the current embedded
architecture and [docs/audio-and-voice.md](docs/audio-and-voice.md) for the
voice pipeline details.
