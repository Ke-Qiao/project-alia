# Alia patches to Extism 1.41.0

Source: the published crates.io `extism` 1.41.0 archive (BSD-3-Clause).

`src/plugin.rs`, `Plugin::raw_call`: reject a guest error above 4096 bytes
before `memory_str`, `to_string` or tracing. The runtime additionally disables
guest tracing and caps successful output before copying or decoding it.

`src/manifest.rs` gates its HTTP-only `Read` import with `register-http`, which
Alia disables along with the SDK's other default features.

The host deliberately creates a new Engine and Plugin for every invocation.
No timer, compiled module, instance pool or mutable guest state is reused.
