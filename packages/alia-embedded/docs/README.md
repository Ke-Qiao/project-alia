# alia-embedded Docs

`alia-embedded` is the edge runtime for Raspberry Pi and local development. It
owns local audio I/O, wake word detection, VAD evidence, scoped wake cues,
binding, discovery, settings, Playback v3, signed MotionControl, plugin services,
and modular sensor providers. VoiceSession v2 is the only speech input path.

## Start Here

- [Architecture](architecture.md) explains the embedded runtime, module slots,
  storage boundaries, lifecycle, and failure model.
- [Configuration](configuration.md) explains `embedded.toml`, SQLite-backed
  runtime state, local development mode, model paths, and restart boundaries.
- [Audio and voice](audio-and-voice.md) explains capture/playback, wake word,
  VAD evidence, scoped native mixing, pause/cancellation, and debug WAVs.
- [Neck motion assets](motion-assets.md) documents migrated neck matrix and
  `.anim` files plus the native animation expression action.
- [Sensors](sensors.md) explains the embedded `SensorRegistry`, provider model,
  gRPC service, and server mirroring flow.
- [YuNet vision](vision-yunet.md) covers Raspberry Pi OpenCV setup, model
  placement, runtime checks, and face tracking tuning.
- [gRPC services](grpc-services.md) summarizes the embedded-hosted services and
  how they fit with the server-hosted VoiceSession contract.

## Boundaries

- Cross-package architecture stays in the root [project docs](../../../docs/README.md).
- Server runtime details live in [alia-server docs](../../alia-server/docs/README.md).
- Shared protobuf contracts live in `crates/proto/proto/`.
