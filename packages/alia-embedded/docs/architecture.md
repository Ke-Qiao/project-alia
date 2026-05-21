# Embedded Architecture

`alia-embedded` is an edge process, not a no-std firmware image. It is written in
Rust and targets Raspberry Pi/Linux first, while keeping local development on
macOS and Windows practical where the selected backends are available.

## Responsibilities

- keep wake word, endpointing, and immediate audio feedback local
- capture microphone audio and play speaker output
- run sherpa-onnx wake word, VAD, and optional edge STT
- play direct response clips for wake word, cancel, fallback, listening, and thinking cues
- stream captured utterances to `alia-server`
- receive server TTS audio streams and play them with interruption support
- expose settings, binding, plugin, audio, expression, and sensor services over gRPC
- run local YuNet face tracking and feed face-center targets into native motion
- store runtime-owned state in SQLite while keeping large assets on disk

## Module Slots

Embedded module selection is represented by `EmbeddedConfig.modules` with
compiled defaults when the config file is missing.

| Slot | Typical implementation | Purpose |
| --- | --- | --- |
| `wakeword` | `sherpa_onnx` | Keyword spotting. |
| `edge_stt` | `sherpa_onnx` | Local speech recognition for transcript/debug behavior. |
| `vad` | `sherpa_onnx` | Voice activity detection and endpointing. |
| `direct_response` | `keyword` | Local response matching and audio playback. |
| `audio_capture` | `cpal` or ALSA fallback | Microphone input. |
| `audio_playback` | `cpal` or ALSA fallback | Speaker output. |
| `mixer` | `priority` | Playback priority and interruption behavior. |
| `discovery` | `mdns` | Device discovery and binding support. |
| `face_tracker` | `yunet` | OpenCV YuNet face center tracking for neck motion. |

## Runtime Lifecycle

1. Load config or compiled defaults.
2. Open SQLite runtime state.
3. Restore identity, binding state, volume, and direct response metadata.
4. Build module context and shared registries.
5. Start audio capture/playback backends.
6. Start wake word, VAD, direct response, and speech flow modules.
7. Start gRPC services on the configured bind address.
8. Announce discovery for server binding.

## Storage Boundaries

SQLite stores small structured state:

- binding state
- volume settings
- direct response rules and imported metadata
- future device-local settings that frontend/server can update

Disk stores large or user-managed assets:

- sherpa-onnx models
- direct response audio files
- audio debug WAV captures
- plugin bundles
- key files and runtime artifacts

## Failure Model

Missing optional assets should log and degrade, not panic. If a component cannot
initialize, the related channel or feature should be disabled where possible
while the process keeps running. Concrete sensor modules should report read
errors through the sensor service instead of crashing the gRPC server.
