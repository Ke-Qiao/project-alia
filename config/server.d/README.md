# Server Config Fragments

`server.toml` is loaded first. All `.toml` files under this directory are then
loaded recursively in lexicographic order and concatenated into one logical
configuration.

Use numeric prefixes to make ordering obvious. Future plugin configs can live
under `plugins/`, for example `plugins/zigbee.toml`.
