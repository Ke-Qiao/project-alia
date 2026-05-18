# Voice Pipeline

The current voice pipeline is built for a practical half-to-full-duplex
assistant: wake word starts a turn, direct response gives immediate feedback,
the server handles heavier reasoning, and barge-in can interrupt playback.

Package-specific implementation details live in the server and embedded docs:

- [server pipeline/runtime docs](../../packages/alia-server/docs/architecture.md)
- [embedded audio and voice docs](../../packages/alia-embedded/docs/audio-and-voice.md)

## End-to-End Turn

1. Embedded captures microphone frames continuously.
2. sherpa-onnx wake word detection triggers the local speech flow.
3. Embedded plays an optional direct response clip for the matched action.
4. Embedded waits for the direct response timing and captures the user utterance
   with VAD-aware endpointing.
5. Embedded may run edge STT for local transcript/debug behavior.
6. Embedded streams the captured audio to the server audio ingress.
7. Server STT converts the audio into text.
8. The pipeline loads conversation, persona, memory snippets, skills, tools, and
   policy context.
9. Router chooses cache, local LLM, remote LLM, or skill-routed execution.
10. LLM output is interpreted as text plus native or parsed tool calls.
11. Tool results are fed back into the LLM loop until the turn is complete.
12. The final assistant text is sent to the channel and synthesized by TTS for
    voice channels.
13. TTS audio is streamed back to embedded for playback.
14. Conversation messages and traceable agent events are persisted.

## Direct Response Layer

Direct responses are not assistant answers. They are local interaction feedback:

- wake word acknowledgement
- cancel or "stand down" response
- fallback when server connection is unavailable
- listening start and listening continue prompts

If a response is missing, embedded should continue the flow. This keeps local
asset mistakes from breaking wake word capture or server communication.

## Interruption and Barge-In

When wake word is detected during assistant playback:

1. Embedded stops playback locally.
2. Embedded sends `InterruptActiveTurn` to the server.
3. The server cancels the active voice turn for that channel/user.
4. Streaming TTS and downstream delivery stop as soon as cancellation is
   observed.
5. In-flight tools are allowed to finish, but their result is discarded if the
   turn was already interrupted.
6. Interrupted assistant text is not persisted as a completed reply.
7. Embedded starts a fresh capture window for the new user utterance.

This behavior is channel-local. Other future channels may choose different
interrupt semantics.

## Continuous Dialogue

After one wake word, embedded can keep a follow-up listening window open after
assistant playback. The user can continue without repeating the wake word until
the follow-up window times out or the configured follow-up turn limit is reached.

This is not the same as always-on recording. The state is explicitly entered by
the wake word flow and bounded by VAD and timeout policy.

## Streaming and Latency

The system has streaming hooks at several boundaries:

- gRPC upload of captured audio from embedded to server.
- LLM token streaming where the selected provider supports it.
- TTS streaming where GPT-SoVITS returns chunks.
- Channel audio streaming back to embedded playback.

The pipeline still keeps correctness fallbacks. If a channel cannot handle
streamed audio, it can buffer and play the complete response.

## Current External Providers

- Embedded wake word, edge STT, and VAD: sherpa-onnx.
- Server STT: whisper.cpp HTTP server.
- Server LLM: OpenAI-compatible provider, Ollama, or LM Studio-compatible
  OpenAI API endpoint.
- Server TTS: GPT-SoVITS HTTP API.
- Memory: Mem0 or simple in-memory fallback.
