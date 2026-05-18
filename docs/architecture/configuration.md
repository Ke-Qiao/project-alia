# Configuration Model

Project Alia separates startup configuration, runtime settings, and runtime
state. The split is important because the frontend will edit settings while the
system is running, but some choices still require rebuilding modules or
reopening hardware.

## Configuration Layers

| Layer | Owner | Examples |
| --- | --- | --- |
| Startup config | Human/admin, deployment tooling | provider selection, channels, database, bind addresses, model paths |
| Runtime settings | Frontend/server APIs | provider endpoint tuning, volume, voice profile fields, policies |
| Runtime state | The application | binding status, imported direct response metadata, latest sensor readings |
| Disk assets | Human/admin or setup scripts | audio clips, sherpa models, provider model files, debug WAVs |

## Package Docs

Detailed configuration docs live beside the owning package:

- [Server configuration](../../packages/alia-server/docs/configuration.md)
- [Embedded configuration](../../packages/alia-embedded/docs/configuration.md)

Root docs only describe the cross-system model.

## Server Summary

The server uses a base `config/server.toml` plus sorted fragments under
`config/server.d/**/*.toml`. The base file owns provider selection and channel
topology. Fragments own provider, policy, profile, resource, and future plugin
settings.

Provider selection is intentionally startup-oriented. Provider parameters and
policy values can often be hot-updated, but switching provider implementations
usually requires rebuilding the module graph. Core registries and state
machines are fixed services, not startup-selectable slots.

## Embedded Summary

The embedded node can boot from compiled defaults when `config/embedded.toml` is
missing. User-editable settings belong in the config file. Runtime-owned
structured state belongs in embedded SQLite. Large assets such as direct
response audio and sherpa-onnx models stay on disk.

## Hot Reload Direction

Runtime setting updates should report:

- changed keys
- keys applied live
- keys requiring restart
- keys ignored by current modules
- per-module apply errors

This lets the frontend safely update settings without pretending every knob can
be hot-reloaded.

## Restart Boundaries

Restart is usually expected for:

- server or embedded bind addresses
- database and auth key material
- provider implementation selection
- channel topology changes
- model paths requiring provider reinitialization
- audio backend/device changes requiring hardware reopen
- runtime directory layout
