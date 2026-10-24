# Embedded Configuration

Normal `alia-embedded` startup uses compiled defaults when `config/embedded.toml`
is missing; selected backends still need their build features, models, and devices.
User-editable settings live in the config file; runtime-owned state is stored in
SQLite. The explicit configuration check below instead requires an existing file.

## Config Path

By default, the binary looks for:

```text
config/embedded.toml
```

Override with:

```bash
export ALIA_EMBEDDED_CONFIG_PATH=config/embedded.toml
```

To inspect a candidate without starting the runtime:

```bash
cargo run -p alia-embedded -- --check-config /absolute/path/embedded.toml
```

The application check reads the configuration and applicable motion files. It
does not open audio/camera/actuator devices, load inference models, create
identity or state files, or start listeners. Missing configuration and unknown
or incomplete arguments fail; they never fall through to normal startup. Cargo
may compile normal build artifacts before running the check. Successful
validation does not establish hardware readiness, native audio formats, model
availability, permissions or calibration.

Use [`embedded.staging.example.toml`](../../../config/embedded.staging.example.toml)
for preparation with device modules disabled. Running that profile normally
still creates runtime state and starts its configured service; only the check
path above is a read-only application operation. Keep field-specific identities
and deployment values in the [central field record](../../../docs/operations/hardware-deployment-readiness.md).

## Main Sections

The general example is `config/embedded.example.toml`. Its default native audio
modules use `auto` / `default` device selection, and normal startup opens playback
and microphone capture before a wake word. Other selected modules can initialize
camera, sensors or discovery; `motion.hardware_enabled=false` only disables
native actuator output. This general example is not a no-device preflight profile.

| Section | Purpose |
| --- | --- |
| `[runtime]` | Provisioning state. |
| `[device_runtime]` | Instance-local bind address, state/plugin paths, and shutdown budgets. |
| `[dev]` | Explicit development binding bypass. |
| `[identity]` | Device id, friendly name, keypair path. |
| `[modules]` | Module implementation choices. |
| `[audio]` | Capture/playback backend, devices, rates, buffers, and volume defaults. |
| `[wakeword]` | sherpa-onnx keyword spotting model and keyword config. |
| `[r1]` | Explicit R1 control UART path for natural wake or explicit cold-boot initialization (Linux). |
| `[speech]` | Wakeword/system trigger, exact Server channel instance ID, VoiceSession pre-roll, and bounded upload queue. |
| `[vad]` | Rust Silero streaming activity evidence settings. |
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

The bound `server_endpoint` is the Server HTTP base address. Embedded derives
the VoiceSession port as that HTTP port plus one. A custom `server.grpc_port`
or forwarded port mapping must preserve this relationship for Embedded voice;
binding and settings checks alone do not verify that audio endpoint.

The other settings are `pre_roll_ms` (default 200) and `upload_queue_frames`
(default 32). The sole path requires mono 16 kHz capture and an available VAD;
there is no `speech.enabled` toggle, `mode`, edge STT, local interrupt, thinking,
or continuous-dialogue section. `[modules]` has no `mixer` or `edge_stt` slot. Native playback always
mixes scoped voice/cue sources at fixed gains 1.0/0.2.

Select `modules.vad = "silero"`. The Rust VAD retains a fixed input window and
recurrent model state; continuous speech no longer triggers a 15-second local
reset or session cancellation. It uses the existing k2-fsa Silero v4 model at
16 kHz with 512-sample windows and CPU inference. Unsupported model schemas or
configuration values fail startup rather than silently choosing another model
or execution provider. The default `sherpa-onnx` Cargo feature supplies both the
Sherpa wake engine and the shared ONNX Runtime used by `silero`.

Migrate old configurations by replacing the VAD module name `sherpa_onnx` with
`silero` and removing `[vad].max_speech_duration_ms` and `buffer_size_sec` from
the instance configuration and persisted settings. These fields and the old VAD
module name are rejected. The wakeword module name `sherpa_onnx` is unchanged.

The Server still bounds a single pending model window to 30 seconds, including
pre-roll and endpoint silence. Exceeding that separate limit closes the session;
this VAD change does not truncate speech, create a synthetic turn boundary, or
extend the model contract.

Set `modules.audio_capture = "noop"` for a service-only host with no microphone.
The speech worker then stays inactive with degraded health; do not select a
Noop VAD while claiming real capture is active. Choose `audio_playback = "noop"`
when output is intentionally disabled. Noop backends do not prove audio readiness.
The staging example also disables wake/VAD and the other device modules. Use the
[audio guide](audio-and-voice.md#capture-and-playback) for explicit Pi ALSA device
selection and the distinction between requested speech format and native format.
Embedded ALSA availability/health checks may open PCM handles; they are not a
device-free configuration inspection.

Production voice also requires the configured server Easy-Turn provider. A
failed provider or interrupted stream requires a fresh wake, not a fallback to
local utterance/STT. Current runtime policies and shadow defaults remain unchanged.

## R1 with Existing Firmware

For system-controlled capture use the separate
[`embedded.r1-system.example.toml`](../../../config/embedded.r1-system.example.toml).
`speech.trigger = "system"` disables wake-based session admission; PCM stays
local until authenticated CaptureControl.Start. The default remains `wakeword`.
The explicit `--initialize-r1 <config> <cold-boot-id>` startup is the only path
that sends one fixed beam-0 manual wake, after opening native capture. There
is no `manual_wakeup`/beam auto-start configuration. See the
[startup, durable claim and recovery contract](audio-and-voice.md#r1-system-controlled-capture).

The following section describes the unchanged natural-wake preset.

[`embedded.r1.example.toml`](../../../config/embedded.r1.example.toml) selects
`modules.wakeword = "r1"` and an explicit `[r1].serial_path`. This Linux backend
uses the existing UART_REAL firmware at 115200, raw 8N1 with flow control off.
It answers device handshakes and queries the firmware version; it does not send
manual wake, beam, wake-word, AEC, firmware or raw-audio configuration commands.
The serial path has no automatic default. Confirm the physical adapter before
using the example's CH340 by-id path, especially with multiple identical adapters.

R1 UART supplies wake notifications; existing ALSA capture supplies processed
16 kHz mono PCM from `hw:CARD=L6Microphone,DEV=0`. The firmware's current wake
word and audio gating still apply (the observed keyword is often
`xiao3-fei1-xiao3-fei1`). No host KWS model is needed in this mode, but continuous
voice still requires the configured VAD model and bound Server voice services.
For the existing Silero provider, `bash scripts/download-sherpa-models.sh --vad-only`
downloads only VAD into `config/models/silero-vad/` without a KWS model or Python
CLI. Set `MODELS_DIR` to the private instance's `models` directory when copying
the configuration elsewhere; the model path is relative to that configuration.
The downloader preserves an existing VAD model. Record the deployed model's hash;
the download URL alone is not an immutable model identity.

The example enables C-Media playback at `hw:CARD=Device,DEV=0`, 48 kHz stereo,
with `alsa_playback_lock_to_capture_rate=false`. Existing playback resamples
incoming voice to the actual output format. Capture and playback retain separate
USB clocks; a connected analog reference does not establish synchronization or
prove AEC performance. Initial gains are 1.0; existing instances restore their
persisted volume settings. Direct-response cues, motion, vision and discovery
remain disabled.
Normal startup opens the control UART, microphone and playback device before pairing; the offline
`--check-config` command opens none of these and does not prove the device or model is available.

One Rust worker owns the UART. A matching version response establishes control
health; connection loss clears pending events. A new handshake after verification
also revokes health until reconnection verifies the version again. A version
query has a two-second deadline and reconnection waits one second; first readiness
can take more than one attempt. A capture reset during initial verification
replaces the discarded query with a new ID without extending that deadline.
A single pending wake expires
after one second of host monotonic time and is consumed only by the existing
speech worker with PCM. Startup, capture loss and session termination reset the
queue and fence in-flight serial input. The event keeps the original message ID,
optional angle and beam; missing values remain absent, and the board's score is
not reported as confidence. The observation timestamp is host UNIX time, not the
PCM sample clock. Direction metadata does not issue motion commands.

Closing the module cancels and joins the UART worker before reporting completion.
Serial open clears DTR/RTS after obtaining the descriptor; driver-specific line
transitions during open are not an electrical guarantee. Input and output audio
devices remain independently clocked; this backend does not establish AEC quality
or remove the firmware's audio gating.

### Opt-in R1 Hardware Fixtures

These Linux-only tests are ignored by ordinary test runs. Arrange the microphone
or speaker window first and use a dedicated private configuration with the
confirmed devices. They perform real device I/O; `--check-config` alone does not.
The commands below run from the repository root with the default `sherpa-onnx`
feature and an optimized test build, matching the production timing profile.
Keep the configuration paths absolute.

For capture, copy the R1 example and set `modules.audio_playback = "noop"`.
Keep direct responses, motion, vision, mmWave and discovery disabled, and
`dev.skip_binding = false`. The confirmed CH340 path is
`/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0`; capture uses
`hw:CARD=L6Microphone,DEV=0` at 16 kHz mono. Provide the local Silero VAD file.
The fixture replaces identity, SQLite and listeners with private temporary
state and loopback endpoints. It opens UART and capture, but never playback:

```bash
ALIA_R1_HARDWARE_CONFIG=/absolute/path/private/capture.toml \
ALIA_R1_HARDWARE_TIMEOUT_SEC=120 \
cargo test --release --locked -p alia-embedded --lib \
  module::speech::duplex::tests::r1_hardware::real_r1_uart_alsa_silero_authenticated_voice_and_rewake \
  -- --exact --ignored --nocapture --test-threads=1
```

`ALIA_R1_HARDWARE_CONFIG` is required. The timeout defaults to 120 seconds and
accepts only 1–120; this bounds the test body, not compilation or cleanup.
Wait for each `await_natural_wake_then_short_phrase_and_silence` log, then say
the firmware's natural wake word and a short phrase, followed by silence.
The two rounds verify UART wake, real PCM and local VAD, a normally signed
VoiceSession Open and upload to a local protocol fixture, and a fresh session
after an intentional fixture disconnect. No authentication bypass, external
chat/STT/Easy-Turn/TTS model or model response is used. Direction comes from the
natural wake event; the test sends no manual wake or beam command. It prints
counts, peak and VAD statistics, and does not save PCM or WAV files.

For output, use the R1 example's playback selection: `cpal` module, explicit
ALSA `hw:CARD=Device,DEV=0`, 48 kHz stereo, independent clocks and a positive
initial playback volume. This fixture constructs only playback; it does not
open the configured microphone, UART or VAD:

```bash
ALIA_R1_PLAYBACK_TEST_CONFIG=/absolute/path/private/playback.toml \
cargo test --release --locked -p alia-embedded --lib \
  module::cpal_audio::hardware_tests::r1_cmedia_playback_host_boundaries \
  -- --exact --ignored --nocapture --test-threads=1
```

`ALIA_R1_PLAYBACK_TEST_CONFIG` is required. A 0.2-second, low-amplitude 440 Hz
synthetic tone passes through the existing resampler, stereo output and software
gain, capped at 0.05. The test checks actual format negotiation, held-silence and
post-mix reference, resume, cancellation and final submission boundaries; it
saves no PCM. Its control evidence is synthetic and its body has a ten-second
deadline. `AlsaWriteAccepted` proves host submission, not DAC presentation,
physical tail length or acoustic AEC.

Both fixtures retain cleanup ownership on ordinary errors, panic and their
internal timeout, cancelling and joining their workers. Native I/O can block
beyond an async timeout, so deployed runs also need process supervision. The
retained `run/r1-complete-20261021/supervise.py` provides the concrete harness:
copy it into a new private run directory with `bin/alia-embedded`, the matching
Linux lib-test executable named `bin/alia-r1-tests`, `private/uart.toml`,
`private/capture.toml`, `private/playback.toml`, and an empty `logs/` directory.
Invoke `python3 /absolute/path/new-run/supervise.py capture` or `playback`;
`uart` runs the standalone runtime with audio modules disabled. The harness
sets the environment variables above, refuses an already visible device owner,
records artifact identity and device snapshots, and refuses to overwrite logs.
Its outer limits are 60 seconds for UART, 140 for capture and 25 for playback;
it then sends SIGTERM, waits up to 12 seconds, and uses a final kill with a
five-second wait if necessary. A fixture watchdog expiry is a failure even if
the OS releases the devices. Preserve that failure and verify the owned process
and visible UART/PCM owners are gone; forced exit is not evidence of completed
module cleanup. Actual acceptance results belong in the
[release record](../../../docs/operations/software-release.md).

## Runtime State

Resource paths are resolved relative to the config file directory. Runtime-owned
state and cue assets commonly use:

```text
run/embedded-state.sqlite3
run/direct-responses/
```

Audio evidence has no default recording directory. Its explicit caller chooses
a new export path; the synthetic replay example does not record real devices.

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

The animation catalog and cue directory restrictions apply to the Motion and
Settings data APIs. WASM processors run without filesystem, network or device
host functions. Explicitly authorized `trusted_native` device drivers run with
the host account's permissions; these data API restrictions do not isolate
files from that account's code. Both runtimes require the reviewed asset digest
at startup; native drivers require separate authorization. See
[WASM deployment](../../../docs/operations/wasm-plugins.md) and
[coordinated upgrade and recovery](../../../docs/operations/upgrade-recovery.md).

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
