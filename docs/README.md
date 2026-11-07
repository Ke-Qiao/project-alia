# Project Alia Documentation

This directory is the current documentation set for Project Alia. The files in
`docs/legacy/`, when present, are historical notes only; prefer the documents
below when trying to understand the active architecture.

## Start Here

- [Architecture overview](architecture/overview.md) explains the system shape,
  packages, shared crates, and the main runtime boundaries.
- [Embodiment](architecture/embodiment.md) describes physical observations,
  bounded Motion, persistent operations, and the boundary with Expression.
- [Voice pipeline](architecture/voice-pipeline.md) follows a wake word turn from
  embedded capture through server STT, LLM, TTS, and playback.
- [Configuration](architecture/configuration.md) explains how static config,
  runtime settings, and package-local configuration docs fit together.
- [Protocols](architecture/protocols.md) summarizes REST, WebSocket, and gRPC
  responsibilities.
- [Motion contract](architecture/motion-contract.md) defines short physical
  actions, signed device evidence, calibration and coordinated upgrade requirements.
- [Plugin contract](architecture/plugin-contract.md) defines extension types,
  trust, management, content/instance identity and lifecycle targets, including
  installation and isolated execution of unreviewed WASM plugins.

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
  running the server and embedded node on a development machine; replacement frontends are planned in the root README.
- [WASM plugins](operations/wasm-plugins.md) covers Rust guest builds, exact-pin grants,
  private Appraisal approval, resource budgets, and Server / Embedded lifecycle.
- [MCP tools](operations/mcp-tools.md) covers service configuration, credentials,
  catalog changes, administration, execution receipts, and transport limits.
- [Software verification](operations/software-release.md) records the current
  regression results and the separate model and hardware acceptance boundaries.
- [Model services](operations/model-services.md) lists the external model
  services currently expected by the runtime.
- [Hardware and deployment](operations/hardware-deployment-readiness.md) records
  the Pi/R1 evidence, board firmware requirements, and remaining hardware inputs.

## Tracking

- [Project TODO](todo.md) is the current entry point for completed software work,
  delivery checks, open investigations, and explicitly deferred work.
- [Upgrade and recovery](operations/upgrade-recovery.md) covers coordinated
  Server / Embedded upgrades, state backups, and failure recovery.
- [Refactor plan](refactor/plan.md) records the agreed scope and proposed stages
  for the server / Raspberry Pi 5 rebuild. It is a plan, not implemented behavior.
- [Deferred work](refactor/deferred.md) records postponed frontend and general
  embedded support and historical decisions, with conditions for resuming them.

## Documentation Scope

Root documents intentionally stay at project and cross-package level. Package
documents own package-specific architecture and operations. Future developer
docs can cover exact APIs, tests, migrations, and provider internals without
turning the overview into a maze with a name tag.
