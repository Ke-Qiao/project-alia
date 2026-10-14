# Embedded Configuration

`alia-embedded` uses compiled defaults when `config/embedded.toml` is
missing; selected backends still need their build features, models, and devices. User-editable settings live in the config file; runtime-owned state is
stored in SQLite.

## Config Path

By default, the binary looks for:

```text
config/embedded.toml
```

Override with:

```bash
export ALIA_EMBEDDED_CONFIG_PATH=config/embedded.toml
```

## Main Sections

The example file is `config/embedded.example.toml`.

| Section | Purpose |
| --- | --- |
| `[runtime]` | Provisioning state. |
| `[device_runtime]` | Instance-local bind address, state/plugin paths, and shutdown budgets. |
| `[dev]` | Explicit development binding bypass. |
| `[identity]` | Device id, friendly name, keypair path. |
| `[modules]` | Module implementation choices. |
| `[audio]` | Capture/playback backend, devices, rates, buffers, and volume defaults. |
| `[wakeword]` | sherpa-onnx keyword spotting model and keyword config. |
| `[speech]` | Exact Server channel instance ID, VoiceSession pre-roll, and bounded upload queue. |
| `[vad]` | sherpa-onnx/Silero activity evidence settings. |
| `[direct_response_defaults]`, `[[direct_response]]` | Local-only cue asset directory and cue metadata. |
| `[hmmd_mmwave]` | Optional Waveshare HMMD UART presence sensor. |
| `[motion]` | Native neck mapping, local-only `approved_animations` content pins (empty by default), expression actions, and serial/PCA9685 servo output. |
| `[vision]` | OpenCV YuNet camera tracking parameters. |
| `[discovery]` | mDNS/BLE advertisement and binding discovery. |

## Voice Configuration

The obsolete `[audio_debug]` source-WAV recorder has been removed. Its table is
rejected by file loading and settings updates, including `enabled = false`;
remove that table from local configuration before starting a new version.
Native dual-stream evidence uses an explicitly owned, bounded software session,
not an automatic recording configuration. See [audio evidence](audio-and-voice.md#debugging-and-limits).

`[speech].channel_id` names the exact `[[channels]].id` on the bound server.
The compiled default `"embedded"` means an actual instance with that name, not a
channel-type alias. The shipped examples use `"pi-living-room"` on both sides.
IDs must contain visible ASCII characters without whitespace; invalid values are
rejected rather than trimmed or converted to another route. Changing this
setting requires restart. Binding does not choose or rewrite the instance ID.

The other settings are `pre_roll_ms` (default 200) and `upload_queue_frames`
(default 32). The sole path requires mono 16 kHz capture and an available VAD;
there is no `mode`, edge STT, local interrupt, thinking, or continuous-dialogue
section. `[modules]` has no `mixer` or `edge_stt` slot. Native playback always
mixes scoped voice/cue sources at fixed gains 1.0/0.2.

Set `modules.audio_capture = "noop"` for a service-only host with no microphone.
The speech worker then stays inactive with degraded health; do not select a
Noop VAD while claiming real capture is active. Choose `audio_playback = "noop"`
when output is intentionally disabled. Noop backends do not prove audio readiness.

Production voice also requires the configured server Easy-Turn provider. A
failed provider or interrupted stream requires a fresh wake, not a fallback to
local utterance/STT. Current runtime policies and shadow defaults remain unchanged.

## Runtime State

Resource paths are resolved relative to the config file directory. Runtime-owned
state and cue/debug assets commonly use:

```text
run/embedded-state.sqlite3
run/direct-responses/
run/audio-debug/
```

The config file should not be used as a database. Server/frontend setting
updates use embedded gRPC settings APIs and persist runtime-owned state through
SQLite where appropriate.

`direct_response_defaults.audio_dir` is a local startup setting. The upload
service uses the resolved directory captured when its settings manager was
created, rather than a relative path from the serialized settings response.
Remote settings cannot change its stored spelling, including an absolute or
relative alias of the same directory. Unchanged complete snapshots and unrelated
settings patches remain accepted. To move this directory, stop the process,
edit its local configuration, then restart.

The animation catalog and cue directory restrictions apply to the Body and
Settings data APIs. Plugin installation and startup are software deployment
authority: plugins currently run as the host process user. These restrictions
do not isolate local files from an operator or plugin with that same user’s code
execution permissions. Plugin installation paths and process isolation require
their own review; this change does not alter that existing boundary.

## Restart Boundaries

Many policy values can be updated live through settings. Restart is still
expected for changes that rebuild core resources:

- gRPC bind address and port
- identity keypair path
- module implementation choices
- model file paths that require recreating sherpa-onnx instances
- YuNet model/camera settings that require recreating the face tracker
- audio backend/device choices that require reopening hardware
- runtime directory layout

## Local Development

Local development can skip full binding when explicitly configured. Use that
only on trusted developer machines. Production devices should use the binding
flow and authenticated gRPC requests.

## Raspberry Pi BlueZ Checks

`[modules].discovery = "mdns"` is the normal backend. In that mode mDNS is the
primary advertisement, while `[discovery].ble_provision_enabled = true` starts a
short-range auxiliary BLE provisioning beacon and GATT provisioning service.

The BLE service UUID is `3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f1`.

| Characteristic | UUID | Direction | Purpose |
| --- | --- | --- | --- |
| Network status | `3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f2` | read | JSON status: `idle`, `applying`, `connected`, or `failed`. |
| Network config | `3d4d5f9a-d0b9-4a7f-9f47-9d6cc7b5b4f3` | write | JSON Wi-Fi credentials. |

Network config writes accept:

```json
{
  "ssid": "Home WiFi",
  "password": "wpa2-passphrase",
  "interface": "wlan0"
}
```

The password is not stored in `embedded.toml`; the embedded process invokes
NetworkManager through `nmcli`, letting the OS own the network profile. Hidden
SSID provisioning is intentionally rejected in the fast path for now.

On Raspberry Pi, BLE provisioning depends on Linux BlueZ and the system
Bluetooth adapter. If the log reports a BlueZ startup or advertisement failure,
check the board before changing application code:

```bash
sudo apt update
sudo apt install -y bluez rfkill
sudo systemctl enable --now bluetooth
systemctl status bluetooth --no-pager
rfkill list bluetooth
sudo rfkill unblock bluetooth
bluetoothctl show
```

Expected signs:

- `bluetooth.service` is active.
- `rfkill` does not show the controller as blocked.
- `bluetoothctl show` prints a controller and `Powered: yes`.

If session creation fails, `bluetoothd` or D-Bus is not available to the
process. If no adapter is found, the kernel did not expose the Bluetooth
controller or it is disabled by overlay/firmware settings. If advertisement
registration fails, confirm the adapter supports LE advertising; older BlueZ
builds may also need `bluetoothd` started with experimental features enabled.

For hardware bring-up where BLE is not needed yet, keep mDNS enabled and set:

```toml
[discovery]
ble_provision_enabled = false
```

### Cue upload write boundary

Uploads with nonempty `audio_data` accept an empty name (a fresh UUID filename)
or a single filename. Absolute paths, nested paths and traversal are rejected
before writing any variant. Uploaded files are new-only: an existing file,
symlink or hardlink is rejected rather than overwritten. Use a fresh filename
or leave it empty when replacing a rule's audio; the returned path identifies
the new file. Existing references with empty `audio_data` remain references and
do not write files.

On macOS/Linux every directory component is opened relative to its parent with
no-follow flags, including the configured root and the sanitized rule slug.
The local startup path must therefore not pass through symlink aliases (for
example use `/private/var/...` instead of `/var/...` on macOS). Trusted startup
`..` components are traversed using directory descriptors; request names cannot
supply them. The upload root is the resolved startup snapshot, not a path from
subsequent settings patches. Unsupported platforms reject uploads.

A failed file batch removes files created by that batch; it may leave empty
created directories. Completed uploads are retained when the later database
operation fails or is cancelled, because its commit may already have happened.
Filesystem and database publication are not one atomic transaction; unused
uploaded files require operator cleanup after checking stored references. No
background file deletion or new task ownership was introduced. File byte writes
are synchronous within their owning call and do not claim a wall-clock bound.
