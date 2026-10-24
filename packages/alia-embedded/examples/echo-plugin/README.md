# Rust WASM echo processor

Build and package the shared [Rust PDK example](../../../../examples/wasm-plugins/README.md).
This directory supplies the Embedded manifest; add the compiled artifact as
`module.wasm` to the archive. Installation never starts it. Activation requires
reviewing the returned content digest and supplying it as `expected_asset_sha256`.
The processor has no native device, network, filesystem or environment authority.

Send `action: "echo"` with JSON payload, the current instance ID and a stable
command ID. An accepted command initially has `unknown` status; only the owned
WASM call's decoded response marks completion. Stop closes admission and waits
for the owned WASM workers to finish. Restart/uninstall remains blocked during cleanup.
For replacement: stop, observe `Stopped`, uninstall, install the new version and
review its new digest. Native sensor/actuator drivers instead require an explicit
`trusted_native` manifest and `allow_trusted_native` operator grant; this example
never uses that mode.
