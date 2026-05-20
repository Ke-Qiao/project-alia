# gRPC Services

`alia-embedded` hosts gRPC services for server control, playback, settings,
binding, plugin management, expression motion, and sensors. Shared protobuf definitions live in
`crates/proto/proto/` and generated Rust code lives in `crates/proto`.

## Embedded-Hosted Services

| Service | Purpose |
| --- | --- |
| `alia.audio.AudioStream` | Server-to-device playback, playback stop, audio upload helpers, and audio control hooks. |
| `alia.settings.EmbeddedSettings` | Settings snapshot, volume, and direct response management. |
| `alia.binding.DeviceBinding` | Device binding and auth handshake support. |
| `alia.plugin.PluginManager` | Embedded plugin/package lifecycle control. |
| `alia.sensor.EmbeddedSensor` | Sensor discovery, reads, and optional sensor commands. |
| `alia.expression.EmbeddedExpression` | Generic expression capabilities, execution, and cancellation routed to native motion. |

## Server-Hosted Services Used by Embedded

Embedded sends captured speech and interrupts to server-hosted audio ingress:

| Service | Purpose |
| --- | --- |
| `alia.audio.ServerAudioIngress` | Captured utterance upload, sensor-data reports, and active-turn interruption. |

## Auth

Production requests are authenticated through binding state and signed tokens.
Local development can bypass auth only when explicitly configured.

## Proto Changes

When adding or changing a service:

1. Edit the relevant `.proto` file under `crates/proto/proto/`.
2. Register it in `crates/proto/build.rs`.
3. Expose the generated module in `crates/proto/src/lib.rs`.
4. Implement the service in `packages/alia-embedded/src/grpc/` or the
   appropriate server gRPC module.
5. Wire the tonic service into the server builder.
6. Run workspace checks because proto changes affect both Rust codebases.
