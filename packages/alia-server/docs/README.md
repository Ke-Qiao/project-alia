# alia-server Docs

`alia-server` is the central runtime for Project Alia. It owns the HTTP API,
WebSocket events, channel registry, AI pipeline, provider adapters, persistence,
settings, automation, tools, skills, sensors, and user/session state.

## Start Here

- [Architecture](architecture.md) explains the server core graph, channels,
  pipeline, provider adapters, persistence, and failure model.
- [Configuration](configuration.md) explains `server.toml`, `server.d/`
  fragments, provider selection, runtime settings, and restart boundaries.
- [Channels and sensors](channels-and-sensors.md) explains channel adapters,
  embedded gRPC integration, sensor mirroring, and channel actions.
- [Expression runtime](expression-runtime.md) explains speech style planning,
  multimodal expression plans, and channel expression actions.
- [Runtime systems](runtime-systems.md) explains skills/tools, automation,
  memory, cache, insights, users, auth, and traceable agent history.
- [Plugin storage](plugin-storage.md) describes the server-owned, namespaced
  persistence layer for plugin-private data.
- [Plugin contract](../../../docs/architecture/plugin-contract.md) distinguishes
  current trusted interfaces from future grants, isolated third-party execution
  and lifecycle acceptance requirements.

## Boundaries

- Cross-package architecture stays in the root [project docs](../../../docs/README.md).
- Embedded node details live in [alia-embedded docs](../../alia-embedded/docs/README.md).
- Protobuf contracts live in `crates/proto/proto/`.
