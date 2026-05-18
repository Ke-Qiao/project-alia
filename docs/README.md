# Project Alia Documentation

This directory is the current documentation set for Project Alia. The files in
`docs/legacy/`, when present, are historical notes only; prefer the documents
below when trying to understand the active architecture.

## Start Here

- [Architecture overview](architecture/overview.md) explains the system shape,
  packages, shared crates, and the main runtime boundaries.
- [Voice pipeline](architecture/voice-pipeline.md) follows a wake word turn from
  embedded capture through server STT, LLM, TTS, and playback.
- [Configuration](architecture/configuration.md) explains how static config,
  runtime settings, and package-local configuration docs fit together.
- [Protocols](architecture/protocols.md) summarizes REST, WebSocket, and gRPC
  responsibilities.

## Package Docs

- [alia-server docs](../packages/alia-server/docs/README.md) cover server
  architecture, configuration, channels, sensors, runtime systems, automation,
  tools, skills, users, auth, and persistence.
- [alia-embedded docs](../packages/alia-embedded/docs/README.md) cover the edge
  runtime, audio and voice flow, direct responses, settings, gRPC services, and
  modular sensor providers.

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

## Tracking

- [Server TODO](todo.md) tracks known server-side implementation gaps and
  follow-up work.

## Documentation Scope

Root documents intentionally stay at project and cross-package level. Package
documents own package-specific architecture and operations. Future developer
docs can cover exact APIs, tests, migrations, and provider internals without
turning the overview into a maze with a name tag.
