# Embedded Architecture

`alia-embedded` is a Rust edge process for Raspberry Pi 5. `EmbeddedBuilder`
constructs an instance-local runtime; the binary is a host for that library.
Local development can use the available backends or explicitly disable audio
and hardware slots with `noop`. Hardware adapters remain Pi-first.

## Responsibilities

- capture microphone audio, detect the wake word, and produce local VAD evidence
- open one authenticated `VoiceSession` and continuously upload 16 kHz mono PCM,
  including while assistant output is playing
- let the server own turn decisions, recognized input, and response generation
- admit network playback and local cues under the original boot/session/generation
- mix conversation and local cues in the native output backend
- expose scoped playback, signed BodyControl, settings, binding, plugin, and sensor services
- own local motion, optional face tracking, discovery, and SQLite device state

There is one voice path. Local utterance endpointing, local STT, transcript-based
cancel matching, follow-up timers, and a second active-turn interrupt RPC have
been removed. Server-side Easy-Turn remains the turn provider; this does not move
model inference onto the Pi.

## Module Slots

`EmbeddedConfig.modules` selects implementations. Selected unavailable or unknown
implementations fail initialization; `noop` explicitly disables a slot.

| Slot | Typical implementation | Purpose |
| --- | --- | --- |
| `wakeword` | `sherpa_onnx` | Wake a fresh voice session. |
| `vad` | `sherpa_onnx` | Activity evidence, without deciding turn completion. |
| `direct_response` | `keyword` | Resolve an optional wake cue from configured assets. |
| `audio_capture` | `cpal`, with Linux ALSA backend support | Microphone input. |
| `audio_playback` | `cpal`, with Linux ALSA backend support | Scoped native output and receipts. |
| `discovery` | `mdns` | Discovery and binding support. |
| `face_tracker` | `yunet` | Optional face center tracking. |

Mixing is a fixed part of native playback, not a module slot. It accepts one
conversation source at gain **1.0** and one local cue at gain **0.2**, clamps the
sum, and uses the same output frame clock. Local cues retain the authenticated
Ready scope and native generation. They cannot create a replacement owner;
Hold suppresses their remaining tail, and retirement cancels both sources.

## Runtime Lifecycle

1. Load and validate config; resolve resource paths against its directory.
2. Open SQLite state, restore binding and runtime settings, and load identity.
3. Construct instance-local modules and their cleanup owner.
4. Start output and selected sensor/motion/vision modules.
5. Start capture, then start the speech worker only if the actual backend is available.
6. Host gRPC services and discovery under the runtime cancellation owner.
7. On shutdown, cancel and join speech/network/cue workers, drain services, and
   stop modules within the configured budget. A deadline returns incomplete cleanup.

A `noop` capture host remains available for service-only use with degraded speech
health. An available capture backend still requires an available VAD. A live
session with discontinuous capture, lost binding, full upload queue, or invalid
remote evidence retires its scope and requires a fresh wake.

## Storage Boundaries

SQLite owns small structured state: binding, volume, direct-response metadata,
and runtime settings. Model files, cue WAVs, plugin bundles, keys, and optional
audio-debug captures stay on disk. Each library instance requires distinct
state/key/plugin paths and a non-conflicting bind address.

## Evidence and Deferred Work

Playback receipts distinguish accepted, submitted, and physically presented
samples. Current CPAL/ALSA boundaries report native submission; they do not prove
physical speaker presentation or silence. Hardware AEC, measured acoustic duplex
quality, and general embedded-platform adaptation remain separate work.

The old continuous-dialogue end tool has no replacement yet. A future explicit
input-session close contract must carry authenticated session ownership and
acknowledged capture closure. Playback Fence controls output admission and must
not be treated as proof that microphone upload stopped.
