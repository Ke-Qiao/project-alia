# Embedded Configuration

`alia-embedded` can boot with compiled defaults when `config/embedded.toml` is
missing. User-editable settings live in the config file; runtime-owned state is
stored in SQLite.

## Config Path

By default, the binary looks for:

```text
config/embedded.toml
```

Override with:

```bash
export ALIA_EMBEDDED_CONFIG_PATH=config/embedded.toml
```

## Main Sections

The example file is `config/embedded.example.toml`.

| Section | Purpose |
| --- | --- |
| `[runtime]` | Provisioning/local-dev mode and runtime behavior. |
| `[identity]` | Device id, friendly name, keypair path. |
| `[binding]` | Server binding state and local development auth bypass. |
| `[modules]` | Module implementation choices. |
| `[audio]` | Capture/playback backend, devices, rates, buffers, and volume defaults. |
| `[audio_debug]` | Optional WAV recording for diagnosing capture/playback. |
| `[wakeword]` | sherpa-onnx keyword spotting model and keyword config. |
| `[edge_stt]` | Local ASR language, model paths, and timing. |
| `[vad]` | sherpa-onnx/Silero VAD model and endpoint thresholds. |
| `[direct_response]` | Local response directory and matching behavior. |
| `[continuous_dialogue]` | Follow-up listening windows and turn limits. |
| `[hmmd_mmwave]` | Optional Waveshare HMMD UART presence sensor. |
| `[motion]` | Native neck mapping, animation assets, expression actions, and PCA9685 output. |
| `[vision]` | OpenCV YuNet camera tracking parameters. |
| `[discovery]` | mDNS/BLE advertisement and binding discovery. |

## Runtime State

Runtime-owned state lives under the configured runtime directory, commonly:

```text
run/embedded-state.sqlite3
run/direct-responses/
run/audio-debug/
```

The config file should not be used as a database. Server/frontend setting
updates use embedded gRPC settings APIs and persist runtime-owned state through
SQLite where appropriate.

## Restart Boundaries

Many policy values can be updated live through settings. Restart is still
expected for changes that rebuild core resources:

- gRPC bind address and port
- identity keypair path
- module implementation choices
- model file paths that require recreating sherpa-onnx instances
- YuNet model/camera settings that require recreating the face tracker
- audio backend/device choices that require reopening hardware
- runtime directory layout

## Local Development

Local development can skip full binding when explicitly configured. Use that
only on trusted developer machines. Production devices should use the binding
flow and authenticated gRPC requests.
