# Audio and Voice

The embedded voice path is optimized for responsiveness. Wake word detection,
direct responses, VAD endpointing, and playback interruption happen locally.

## Voice Loop

```mermaid
sequenceDiagram
    participant Mic
    participant Embedded
    participant Server
    participant Speaker

    Mic->>Embedded: Continuous audio frames
    Embedded->>Embedded: Wake word detection
    Embedded->>Speaker: Optional direct response
    Embedded->>Embedded: VAD-guided capture
    Embedded->>Embedded: Optional edge STT
    Embedded->>Server: Captured utterance
    Server->>Server: STT, LLM/tool loop, TTS
    Server->>Embedded: Playback stream
    Embedded->>Speaker: Buffered playback
```

## Capture and Playback

The audio layer supports CPAL and a Linux ALSA fallback path. On Raspberry Pi
audio HATs, capture/playback device behavior may be board-specific. Useful
settings include:

- capture device/backend selection
- playback device/backend selection
- sample-rate behavior
- ALSA S32 alignment detection
- capture and playback volume
- playback buffer sizing
- debug WAV recording

## Wake Word and VAD

sherpa-onnx handles local wake word detection and VAD. VAD controls when
post-wake capture starts and ends so the system does not cut users off after a
short pause.

## Direct Responses

Direct responses are local feedback, not assistant answers. Supported response
classes include:

- wake word acknowledgement
- cancel or stand-down response
- fallback when server communication fails
- listening start prompt
- listening continue prompt
- thinking cue while waiting for the server
- error cue when server-side TTS or audio delivery cannot speak a full error

All response classes are optional. Missing files should log an error or warning
and continue the voice flow.

The cancel action also has built-in text aliases, including common goodbye and
stop phrases such as "再见", "别说了", and "结束对话". These built-in aliases
are action-only and need no audio file, so saying goodbye can end continuous
dialogue even when the cancel bundle has not been imported.

Direct response assets stay on disk. Metadata is imported into SQLite from
`config.json` bundles under the direct response directory. After successful
import, the bundle config is removed so repeated boots do not re-import it.

The server can request a direct response by generic action name through
`alia.audio.AudioStream.PlayDirectResponse`. This keeps server logic independent
from embedded direct-response storage and lets non-embedded channels ignore or
map the same action differently.

## Interruption

When wake word is detected during playback, embedded should:

1. stop local playback immediately
2. send an active-turn interrupt to the server
3. skip the normal wake word direct response by default
4. start a fresh capture window

This makes barge-in feel natural and avoids speaking over the user.

When captured speech has been uploaded and the device is waiting for server
reasoning, embedded enters a separate server-turn waiting state. If configured,
it plays the `thinking` direct response after a short delay and then at a bounded
interval until TTS starts. A wake word during this waiting state sends
`wakeword_during_thinking` through the existing active-turn interrupt path and
starts a new capture immediately.

## Continuous Dialogue

After a successful wake word turn, embedded can keep a bounded follow-up
listening state active. Users can continue speaking without repeating the wake
word until the follow-up timeout or turn limit is reached.

The state ends when the user says a local cancel direct response phrase, when it
times out, when the turn limit is reached, or when the server sends the generic
`end_continuous_dialogue` channel action. The server action is transported over
the existing playback-control service and does not change `ServerAudioIngress`.

## Audio Debugging

Enable audio debug WAV capture only while diagnosing problems. The files can
grow quickly, but they are valuable for verifying:

- raw microphone capture
- wake word segments
- post-wake capture
- playback payloads
- sample-rate or channel conversion issues
