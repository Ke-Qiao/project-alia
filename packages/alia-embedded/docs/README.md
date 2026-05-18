# alia-embedded Docs

`alia-embedded` is the edge runtime for Raspberry Pi and local development. It
owns local audio I/O, wake word detection, VAD, edge STT, direct responses,
binding, discovery, settings, playback, plugin services, and modular sensor
providers.

## Start Here

- [Architecture](architecture.md) explains the embedded runtime, module slots,
  storage boundaries, lifecycle, and failure model.
- [Configuration](configuration.md) explains `embedded.toml`, SQLite-backed
  runtime state, local development mode, model paths, and restart boundaries.
- [Audio and voice](audio-and-voice.md) explains capture/playback, wake word,
  VAD, direct responses, continuous dialogue, interruption, and debug WAVs.
- [Sensors](sensors.md) explains the embedded `SensorRegistry`, provider model,
  gRPC service, and server mirroring flow.
- [gRPC services](grpc-services.md) summarizes the embedded-hosted services and
  how they fit with server-hosted audio ingress.

## Boundaries

- Cross-package architecture stays in the root [project docs](../../../docs/README.md).
- Server runtime details live in [alia-server docs](../../alia-server/docs/README.md).
- Shared protobuf contracts live in `crates/proto/proto/`.
