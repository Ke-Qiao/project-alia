# Sensors

Embedded sensors are modular. The embedded process owns concrete sensor control
code; the server mirrors definitions and readings into its unified sensor
registry for tools, scheduler, rules, frontend APIs, and future plugins.

## Embedded Registry

Concrete modules implement `SensorProvider` and register with
`ModuleContext::sensors`.

The provider owns device-specific behavior:

- static sensor definition
- read operation
- optional sensor-specific commands

The registry owns discovery and routing. It does not know how each physical or
virtual sensor works.

## Provider Model

Every provider exposes:

- `definition()`: metadata such as id, display name, source, unit, value type,
  capabilities, and metadata
- `read()`: returns a current reading or an error
- `invoke(command, args)`: optional command path for configurable sensors

Sensor ids are local to the embedded node. The server adds the channel namespace
when mirroring them.

## gRPC Service

Embedded hosts `alia.sensor.EmbeddedSensor`:

| RPC | Purpose |
| --- | --- |
| `ListSensors` | Enumerate providers registered in embedded. |
| `ReadSensor` | Read one provider through its concrete implementation. |
| `InvokeSensor` | Execute an optional sensor-specific command. |

Unknown sensors return gRPC `not_found`. A registered sensor that fails to read
returns an unavailable reading with an error message.

## Server Mirroring

The server embedded channel calls `ListSensors` on connection and periodically
while connected. It mirrors definitions into server `SensorRegistry`.

Server-side public ids use:

```text
{channel_id}:{embedded_sensor_id}
```

For example, embedded id `temperature` on channel `pi5` becomes
`pi5:temperature`.

When a sensor disappears from embedded discovery, the server marks it offline
instead of deleting it. This protects existing rules, scheduler tasks, and UI
references.

## Adding a Sensor Module

1. Implement `SensorProvider` in a module that owns the concrete sensor logic.
2. Create a stable local sensor id.
3. Register the provider with `ctx.sensors`.
4. Return read errors as `Result` values instead of panicking.
5. Add capabilities and metadata that help server tools and the frontend explain
   the sensor.

The server side does not need new code for ordinary sensors. It only needs new
code if the sensor requires special channel actions or a new tool.

## Built-In HMMD mmWave Sensor

The Waveshare HMMD-mmWave-Sensor is supported over UART. OT2 is optional and is
not required by the built-in module.

Example Pi 5 wiring:

| HMMD pin | Raspberry Pi 5 pin |
| --- | --- |
| `3V3` | 3.3V, physical pin 1 or 17 |
| `GND` | GND, such as physical pin 6 |
| `TX` | GPIO15 / RXD, physical pin 10 |
| `RX` | GPIO14 / TXD, physical pin 8 |

Enable it in `config/embedded.toml`:

```toml
[hmmd_mmwave]
enabled = true
serial_path = "/dev/ttyAMA0"
baud_rate = 115200
sensor_prefix = "hmmd"
configure_report_mode = true
read_timeout_ms = 200
reconnect_delay_ms = 1000
stale_after_ms = 3000
```

The module registers:

| Embedded sensor id | Value |
| --- | --- |
| `hmmd:presence` | Boolean presence state. |
| `hmmd:target_distance_cm` | Target distance in centimeters when reported. |
| `hmmd:raw` | JSON payload with presence, distance, raw frame bytes, age, and connection state. |

On the server, these are mirrored with the embedded channel prefix, for example
`embedded:hmmd:presence`.
