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
9. Router chooses cache, minor LLM, major LLM, or skill-routed execution.
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
- short thinking cue while embedded is waiting for server-side reasoning or TTS
- short error cue when the server cannot synthesize or deliver a spoken error

The cancel action has a compiled safety net for common goodbye/cancel phrases
such as "再见", "别说了", and "结束对话", so manual continuous-dialogue
shutdown does not depend on importing an audio bundle. User-configured direct
response rules still take precedence over built-in aliases.

If a response is missing, embedded should continue the flow. This keeps local
asset mistakes from breaking wake word capture or server communication.

## Error Notification

Pipeline errors are converted to safe user-facing summaries. Raw provider,
network, stack, and filesystem errors stay in logs and debug events only.

- Text channels receive a text error message.
- Voice channels synthesize a short spoken error summary when TTS is healthy.
- If TTS or audio delivery is the failing component, the server asks the
  channel to play the generic `error` direct response action.

This fallback is intentionally expressed as a channel action
(`play_direct_response`) on the generic channel adapter. The server does not
know embedded direct-response storage, paths, or playback internals, and
`alia.audio.ServerAudioIngress` remains unchanged. Embedded exposes the action
through `alia.audio.AudioStream.PlayDirectResponse`.

Error notifications are recorded as conversation events, not assistant
messages. This avoids saving synthetic failure text as if it were a completed
assistant answer.

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

When wake word is detected while embedded is waiting for server reasoning, the
same interrupt RPC is used with reason `wakeword_during_thinking`. The server
cancels the in-flight generation, preserves the interrupted user question as
context, and does not persist any partial assistant response.

The thinking cue is local audio feedback only. It starts after a short delay,
repeats at a bounded interval, and stops as soon as assistant TTS begins or the
turn is interrupted. `alia.audio.ServerAudioIngress` is unchanged; this uses the
existing active-turn interruption path.

## Continuous Dialogue

After one wake word, embedded can keep a follow-up listening window open after
assistant playback. The user can continue without repeating the wake word until
the follow-up window times out or the configured follow-up turn limit is reached.

This is not the same as always-on recording. The state is explicitly entered by
the wake word flow and bounded by VAD and timeout policy.

There are two ways to end this state manually:

- Embedded local direct response can recognize cancel/goodbye phrases and end
  the session without contacting the server.
- If a goodbye/cancel phrase reaches the server, the generic
  `end_continuous_dialogue` tool asks the current channel to end follow-up
  listening through a channel action. Embedded implements that action through
  existing playback-control RPCs; `alia.audio.ServerAudioIngress` remains
  unchanged.

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
- Server LLM: OpenAI-compatible provider, Anthropic, Gemini, or LM Studio-compatible
  OpenAI API endpoint.
- Server TTS: GPT-SoVITS HTTP API.
- Memory: built-in Alia memory system.
