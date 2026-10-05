# Voice Pipeline

The only device voice path is continuous VoiceSession v2 with Playback v3 and
Body v1. The former utterance pipeline, AudioStream / ServerAudioIngress RPCs,
local edge STT decision path and Expression RPC have been removed.

## Current flow

1. Embedded continuously captures microphone frames. Wake word detection starts
   a session; local VAD supplies evidence without choosing server turn boundaries.
2. The device authenticates its connection, validates Ready and activates that
   exact boot/session/generation with its original capture owner.
3. A bounded local wake cue may start after Ready. Its worker does not block
   microphone capture or upload and cannot adopt a replacement session.
4. Server admission pins the configured active account and source privacy and
   relationship epochs. Streaming voice evidence feeds the server turn detector;
   Easy-Turn remains deployed on the server.
5. A recognized turn uses the existing Context, Interaction and Execution
   contracts. Effects retain their execution ledger and cancellation ownership.
6. Delivery reserves a canonical Response and exact media identities before
   sending output. Embedded playback requires the registered binding; actions
   require the typed Body contract. CLI text uses its owned text writer.
7. Native progress records actual submission boundaries. Dispatch, model return
   and elapsed audio duration never prove physical presentation or human receipt.

## Native mixer and local cues

CPAL and ALSA share a fixed two-source mixer: main voice at gain 1.0 and a short
local cue at gain 0.2. Configured output volume is applied to each source before
native mixing and final peak limiting.
Each source retains its own identity, resampler, frame counters and cancellation.
There is no separate noop mixer module or optional legacy audio writer.

Both sources use the original scope's control gate. Pause closes output admission
and discards the cue's remaining tail; the main response retains its resumable
owner. Fence, capture loss, binding revocation and shutdown retire both sources.
A cue created before Response registration can provide settled native-owner
retirement evidence without being counted as Response progress.

Cue input is bounded to a regular PCM16 WAV file of at most 4 MiB and five seconds.
Admission, End and native submission observation share a seven-second deadline.
Late file completion or cancelled workers cannot start a cue in another scope.

The playback reference is the final mixed native PCM. This supplies the correct
reference location for future AEC; **mixing is not echo cancellation**. Hardware
latency, clock drift and real acoustic echo performance still require R02 work.

## Pause, cancellation and errors

Temporary acoustic yielding and explicit waiting remain separate policies.
An explicit wait resumes only with authorized continuation evidence. Resume
keeps the original producer and native owner; it does not regenerate an answer.

Text and synthesized error notices pass through the same Delivery contracts.
A failed TTS or delivery attempt does not invoke an unscoped device sound RPC.
Interrupted output is not persisted as a completed assistant reply.

The former `end_continuous_dialogue` tool has been removed. Playback Fence closes
output, not the microphone session. A future authenticated command to close an
input session must be designed and verified explicitly; current session shutdown
uses its transport and local lifecycle ownership.

## Scope and evidence

Frontend integration, portable embedded adapters, device-side turn inference,
real AEC and physical actuator evidence remain deferred. Default Shadow policy
and the distinction between software evidence and physical outcomes are unchanged.

- [Protocol and evidence boundaries](../refactor/voice-protocol.md)
- [Embedded audio implementation](../../packages/alia-embedded/docs/audio-and-voice.md)
- [Server runtime](../../packages/alia-server/docs/runtime-systems.md)
- [Cleanup checklist](../refactor/checklists/cleanup-2026-10-06.md)
