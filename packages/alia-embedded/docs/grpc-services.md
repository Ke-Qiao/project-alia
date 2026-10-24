# gRPC Services

Shared contracts live in `crates/proto/proto/`; `crates/proto` generates the Rust
bindings. The embedded library hosts services through its runtime builder.

## Embedded-Hosted Services

| Service | Purpose |
| --- | --- |
| `alia.playback.v3.Playback` | Scoped network audio, output registration/retirement, generation fences, independent Hold/Resume control, and native evidence. |
| `alia.capture.v1.CaptureControl` | Authenticated Inspect/Start/Stop for explicitly selected system-triggered voice sessions. |
| `alia.motion.v1.MotionControl` | Signed, strict request/reply envelopes for capabilities, session-scoped commands, cancellation, and observations. |
| `alia.settings.EmbeddedSettings` | Settings, volume, and direct-response asset metadata. |
| `alia.binding.DeviceBinding` | Binding and authentication handshake. |
| `alia.plugin.PluginManager` | Embedded plugin/package lifecycle. |
| `alia.sensor.EmbeddedSensor` | Sensor discovery, reads, and supported commands. |

There is no generic raw audio stream, direct-response playback RPC, unscoped
stop RPC, or legacy expression execution service. Local wake cues enter the same
native playback owner through the in-process scoped cue API.

The current `PluginManager` controls trusted host-user processes; its binding
token check is not a sandbox or proof of administrator approval for arbitrary
code. The [plugin contract](../../../docs/architecture/plugin-contract.md)
retains separate management authorization and unreviewed-code isolation goals.
No WASI or other sandbox backend is enabled by this refactor.

`InstallPlugin` requires a name and version matching the archive manifest. It
returns the host-computed `asset_sha256`; installation leaves the package
inactive. Archives are limited to 16 MiB, 1,024 members, 16 MiB per file and
64 MiB expanded. Paths, special files, links, duplicate entries and the entry
file are checked before atomic publication. Existing installations cannot be
overwritten. Legacy directories without a host asset record remain inactive;
repackage and explicitly reinstall them.

`ListPlugins` exposes the package digest and current `instance_id`. A command
must include that instance and a stable caller-owned `command_id`. Repeating
the same ID and payload observes the original receipt; reusing it for another
instance or payload is rejected. Server action callers must pass both IDs,
and `install_plugin` callers must pass `version`. The Server
`send_plugin_command` action accepts `payload` as a JSON string; omission defaults
to `{}`, while an explicit non-string value (including `null`) is rejected.

Read the response `state`: a queued command remains `unknown` and must not be
replayed with a new ID, and `success` is true only for `completed`. Events carry
the original instance and command identities. Process completion reports are
plugin assertions, not evidence of physical playback or actuator position.
Stopping closes command admission; only original process/pipe cleanup permits
a stopped state, restart or uninstall. A host restart creates a new instance;
old instance IDs cannot dispatch commands into it.

## Server-Hosted Voice Session

| Service | Purpose |
| --- | --- |
| `alia.voice.v2.VoiceSession` | Bidirectional `Stream`: Open, continuous capture, auth refresh, Close; server Ready, turn evidence, and initiative admission. |

`speech.channel_id` is sent unchanged in both Open and `x-alia-channel-id`
metadata. It must equal the bound server's concrete `[[channels]].id`; a channel
type such as `embedded` is not an alias for another instance.

Embedded sends 16 kHz mono PCM with a contiguous sequence and sample offset.
VAD is activity evidence only. Ready must match the original device boot and
negotiated contract before capture may cause output. Later evidence must retain
its session identity; missing/foreign scope, stale evidence, or binding loss
retires the session rather than silently adopting a new owner.

Voice decisions are semantic requests. Only the independent Playback control
contract can establish Held/Resumed evidence. Playback generation Fence and
output retirement do not themselves establish that the input session closed.
CaptureControl.Stop closes input by exact device boot and local capture generation;
it is separate from Playback Fence. Mapping a spoken tool to it remains future work.

## System Capture Control

All three operations require the current bound `embedded:control` token.
Start/Stop are available only with `speech.trigger="system"`; Inspect exposes
`device_boot_id`, `generation`, `active`, and `available`. Pass the observed
boot and `expected_generation` on each mutation. Stale generations, wrong
boots, unavailable workers and full command queues are rejected. Queued
requests whose callers disappeared are discarded before admission; an RPC
timeout after admission may still mean the operation occurred, so inspect.

Start pins the authenticated binding through local admission and creates the
existing VoiceSession path. It does not initialize R1, change pickup direction,
emit WakeWordDetected or synthesize a Ready response. `active=true` proves only
local session admission. Stop cancels/joins the original session and observes
its native retirement before acknowledging; missing cleanup evidence fails
closed. Its local generation is not the Server VoiceScope generation epoch.

## Authentication and Evidence

Voice admission uses the bound channel/account, signed device token, and current
privacy/relationship epochs. Tokens refresh during the long-lived stream;
revocation and binding replacement end admission. Playback uses the original
scope and registered output binding; MotionControl validates signatures and
request nonces. Development binding bypass remains explicit configuration.

Current native receipts prove CPAL callback-buffer or ALSA-write submission.
`presented_samples` stays absent without supporting physical-device evidence.
No duration estimate or successful gRPC request is a presentation receipt.

## Changing a Contract

1. Edit its `.proto` under `crates/proto/proto/`.
2. Register generated modules in `crates/proto/build.rs` and `src/lib.rs`.
3. Implement the service and wire it through the library runtime builder.
4. Validate both Rust packages and real loopback transport behavior, including
   incompatible identities, cancellation, and lifecycle cleanup.
