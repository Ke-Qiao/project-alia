# Example Server Config Fragments

Copy `server.example.toml` to `server.toml`, and copy this directory to
`server.d/` when creating a runtime config from the example.

Fragments are deep-merged after the base config. Keep provider selection in
`server.toml` under `[modules]`. Core server services are not replaceable slots;
extend them through registries for tools, skills, conditions, actions, sensors,
and plugin-owned prompt/context contributions. Place implementation details by
semantic role:

- `providers/` configures external services and provider-specific adapters.
- `policies/` configures runtime behavior such as routing, cache, and tools.
- `profiles/` configures named user-facing profiles such as voices.
- `resources/` configures local resource locations and resource policies.
- `plugins/` is reserved for future plugin-owned fragments.
