# gRPC Services

Shared contracts live in `crates/proto/proto/`; `crates/proto` generates the Rust
bindings. The embedded library hosts services through its runtime builder.

## Embedded-Hosted Services

| Service | Purpose |
| --- | --- |
| `alia.playback.v3.Playback` | Scoped network audio, output registration/retirement, generation fences, independent Hold/Resume control, and native evidence. |
| `alia.body.v1.BodyControl` | Signed, strict request/reply envelopes for capabilities, session-scoped commands, cancellation, and observations. |
| `alia.settings.EmbeddedSettings` | Settings, volume, and direct-response asset metadata. |
| `alia.binding.DeviceBinding` | Binding and authentication handshake. |
| `alia.plugin.PluginManager` | Embedded plugin/package lifecycle. |
| `alia.sensor.EmbeddedSensor` | Sensor discovery, reads, and supported commands. |

There is no generic raw audio stream, direct-response playback RPC, unscoped
stop RPC, or legacy expression execution service. Local wake cues enter the same
native playback owner through the in-process scoped cue API.

The current `PluginManager` controls trusted host-user processes; its binding
token check is not a sandbox or proof of administrator approval for arbitrary
code. Command responses can mean only sent, and stop requests do not themselves
prove process exit. The [plugin contract](../../../docs/architecture/plugin-contract.md)
defines the target package, run/command identity, management authorization and
cleanup semantics, with unreviewed code installation retained as a future goal.
Those targets have not been implemented by this documentation change.

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
A server-to-device explicit input-session close command remains a recorded TODO.

## Authentication and Evidence

Voice admission uses the bound channel/account, signed device token, and current
privacy/relationship epochs. Tokens refresh during the long-lived stream;
revocation and binding replacement end admission. Playback uses the original
scope and registered output binding; BodyControl validates signatures and
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
