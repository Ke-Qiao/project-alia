# Project Alia Documentation

This directory is the current documentation set for Project Alia. The files in
`docs/legacy/` are preserved as historical notes only; prefer the documents
below when trying to understand the active architecture.

## Start Here

- [Architecture overview](architecture/overview.md) explains the system shape,
  packages, shared crates, and the main runtime boundaries.
- [Voice pipeline](architecture/voice-pipeline.md) follows a wake word turn from
  embedded capture through server STT, LLM, TTS, and playback.
- [Server architecture](architecture/server.md) explains the server module
  graph, channels, providers, registries, and persistence boundaries.
- [Embedded architecture](architecture/embedded.md) explains the Pi/runtime edge
  node, audio loop, direct responses, continuous dialogue, and local storage.
- [Configuration](architecture/configuration.md) explains the layered server
  config, embedded config, runtime settings, and restart boundaries.
- [Protocols](architecture/protocols.md) summarizes REST, WebSocket, and gRPC
  responsibilities.

## Concepts

- [Modules and providers](concepts/modules-and-providers.md) defines modules,
  providers, channels, lifecycle, feature flags, and plugin-facing extension
  points.
- [Skills and tools](concepts/skills-and-tools.md) defines the separation
  between cognitive context and executable actions.
- [Automation](concepts/automation.md) explains rules, scheduler triggers,
  shared conditions, actions, and plugin extension points.

## Operations

- [Local development](operations/local-development.md) gives a short path for
  running the server, embedded node, and frontend on a development machine.
- [Model services](operations/model-services.md) lists the external model
  services currently expected by the runtime.

## Documentation Scope

These documents intentionally stay at framework and technical-detail level. They
avoid function-by-function implementation notes so that future developer docs can
cover exact APIs, test strategy, migrations, and provider-specific internals
without making this overview noisy.
