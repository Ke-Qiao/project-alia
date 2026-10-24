# Rust WASM plugin

Build with `cd examples/wasm-plugins && cargo +1.96.0 build --release --locked`.
The `.cargo/config.toml` selects `wasm32-unknown-unknown` and declares a 16 MiB
linear-memory maximum. Install `target/wasm32-unknown-unknown/release/alia_wasm_demo.wasm`
with `package.json` in the Server WASM package API. Installation is inactive;
review and enable the returned implementation pin. Private Appraisal additionally
requires that exact pin in persistent Appraisal engine policy. An upgrade or a
host binary change invalidates the grant and requires a fresh review.

The guest exports `alia_expression` (domain ABI 3), `alia_tool` (JSON object), and
`alia_processor` (the shared `alia-plugin-api` request/response). It uses the Rust
PDK with default HTTP and MessagePack features disabled. No Python, Bun or WASI
runtime is involved. Every invocation has fresh Engine, variables and memory.

To install the same artifact on Embedded, create a ZIP containing the manifest in
`packages/alia-embedded/examples/echo-plugin/manifest.toml` and the built artifact
renamed `module.wasm`. Install name `echo-plugin`, version `2`, then pass the
returned `asset_sha256` as `StartPluginRequest.expected_asset_sha256`. Submit an
`echo` command with the returned instance ID and a stable command ID. Repeated
commands read their original receipt. Stop waits through the existing lifecycle
owner before a subsequent uninstall or replacement can succeed.
