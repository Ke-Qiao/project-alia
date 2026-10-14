# Audio and Voice

The only device voice path opens a continuous `VoiceSession` after wake word.
Embedded owns capture, VAD evidence, scoped native output, and short local cues.
The server owns turn decisions and recognized-input processing. Easy-Turn runs
on the server; enable its configured provider to admit production voice input.
Set `speech.channel_id` to the exact Server channel instance ID (the examples
use `pi-living-room`); the same value is required in Open and auth metadata.

## Voice Loop

```mermaid
sequenceDiagram
    participant Mic
    participant Embedded
    participant Server
    participant Native as Native output
    Mic->>Embedded: Continuous 16 kHz mono frames
    Embedded->>Embedded: Wake word + bounded pre-roll
    Embedded->>Server: Authenticated VoiceSession Open
    Server->>Embedded: Ready with original scope
    Embedded->>Native: Optional scoped wake cue (gain 0.2)
    loop Session active, including during playback
        Embedded->>Server: Ordered PCM + VAD evidence
        Server->>Server: Easy-Turn evidence and turn policy
    end
    Server->>Embedded: Registered Playback v3 output (gain 1.0)
    Embedded->>Native: Mix scoped sources on one output clock
    Native->>Server: Submission receipts via Playback
```

The cue worker waits for an authenticated Ready and runs independently of
capture and network receive. Each wake admits at most one cue. It keeps the
original scope while loading the file; cancellation or a changed owner rejects
late work. Capture uses bounded `try_send`; overload ends the session rather
than dropping audio and preserving a false contiguous clock.

## Capture and Playback

CPAL and Linux ALSA backends support explicit device selection, fixed output
format, capture/playback volume, bounded buffers, and native timing evidence.
Capture sent to the server is mono 16 kHz; frames include sample offsets and VAD
activity. That configured target is distinct from the negotiated native input:
CPAL selects a supported configuration and ALSA negotiates nearby rates/channels
before software conversion. Playback sources are converted to the selected
fixed native format. Record both the requested values and actual startup/native
evidence; configuration alone does not prove a device's format or channel meaning.

For the Pi field setup, use explicit `audio.capture_backend = "alsa"` and
`audio.playback_backend = "alsa"` only with PCM names verified for the selected
devices. Match the named `CARD` and `DEV` to USB identity, serial or physical path
from enumeration; do not assume a numeric card index remains stable after
reconnect. `default` delegates selection to the host, and CPAL currently selects
the first device whose name contains the configured string. Neither proves that
the intended R1 input or speaker was selected. Keep the actual values in the
[central field record](../../../docs/operations/hardware-deployment-readiness.md)
rather than copying an unverified `hw:0,0` example.

`alsa_playback_lock_to_capture_rate` selects the configured capture rate at
startup for an explicitly confirmed shared-clock design; it does not establish
clock synchronization. Keep its default `false` for the proposed R1 USB capture
and separately selected output chain. USB connection does not prove that R1
offers playback or consumes USB audio as an AEC reference. Playback never retunes
the active capture clock.

The public R1 documentation separates processed mono UAC capture from an external
amplifier reference input; it does not establish USB playback or an internal USB
reference. The actual board/firmware, electrical compatibility and channel meaning
remain unverified; see the [R1 preflight record](../../../docs/refactor/r02-r1-aec-preflight.md).
The current capture converter averages every negotiated native input channel before
conversion. Setting `capture_channels = 1` does not select a processed channel.
A confirmed processed mono endpoint can use this path; an endpoint combining
processed, raw or reference signals needs a verified channel selection first.
Hardware AEC does not automatically require a second host AEC processor or reader.

Normal Embedded startup opens native playback and capture before the wake word;
Linux ALSA availability checks can also open PCM handles. Use the
[read-only configuration check](configuration.md#config-path) and the separate
staging profile during preparation. Do not start the native runtime or query its
audio health as a way to avoid opening the devices.

Native playback has exactly two sources: conversation **1.0** and local cue
**0.2**. The sum is clamped to PCM range. Both retain the original session/control
and native generation. A cue does not consume remote playback sequence numbers
or acquire a replacement output owner. There is no configurable mixer slot.

## Local VAD and Cues

sherpa-onnx handles wake word and VAD. VAD labels activity but never closes a
turn, runs a local transcript, or matches a spoken stop phrase. An optional
`wakeword` action (or the triggering wake keyword) resolves the current cue.
Metadata lives in SQLite and WAV assets stay on disk. Cue records require a
nonempty audio path with finite positive weight; action-only records and built-in
spoken cancel aliases are rejected. Missing cues leave capture running. The
[current wake cue example](../../../config/direct_response.example/README.md)
reuses an existing short repository sample. The decoder accepts bounded PCM16 WAV, averages multichannel input to
mono, and rejects empty/invalid input, clips longer than five seconds, or files
larger than 4 MiB.

The old thinking timer, follow-up window, transcript-based local cancel, and
server-requested fallback/direct-response trigger have been removed. New prompt
uses must be admitted through the same scoped cue path instead of raw playback.

## Pause, Cancellation, and Session Closure

The server distinguishes temporary yielding from explicit pause through the
existing Hold/Resume contract. A voice-evidence decision alone is not an applied
pause receipt. Independent Playback control establishes the actual owner-bound
software Hold. Cues are ephemeral: Hold suppresses the remaining cue tail, so it
is not replayed after conversation resumes. Retirement cancels both sources.

Network failure, capture discontinuity, queue overflow, binding change, invalid
session evidence, or runtime shutdown cancels and joins the network and cue
workers and retires the capture scope. A new connection needs a fresh wake.
There is no separate wakeword interrupt RPC or local follow-up endpointing path.

A future explicit **input-session close** command remains required for a spoken
"end conversation" tool. It must acknowledge authenticated capture closure.
The deleted `end_continuous_dialogue` tool is not mapped to Playback Fence,
which controls output admission and does not stop microphone upload by itself.

## Debugging and Limits

The old source-WAV recorder and `[audio_debug]` configuration have been removed.
It recorded a local cue before mixing/native acceptance and did not establish
dual-stream timing evidence. Remove that obsolete table from local configuration;
file loading and settings updates reject it even when disabled.

R02-A records bounded native input and final output evidence through one exclusive
consumer. Its explicit session and offline replay do not start audio devices,
record automatically, or add AEC. Capture is the data delivered by the backend,
which may already have been processed by device firmware. It is not proof of raw
ADC samples or acoustic capture/playback alignment. Missing reference stays
missing; recorded zero-valued samples are separate evidence.

The `audio_evidence_replay` example has two explicit operations. From the
repository root, choose a new output path whose parent already exists:

```sh
cargo run -p alia-embedded --no-default-features --example audio_evidence_replay --offline --locked -- generate /tmp/alia-synthetic-evidence.json
cargo run -p alia-embedded --no-default-features --example audio_evidence_replay --offline --locked -- replay /tmp/alia-synthetic-evidence.json
```

`generate` publishes the bounded `SyntheticFixture` without overwriting and
reads it back. `replay` only reads an existing ordinary evidence JSON file;
it does not create files or play audio. The old implicit one-path invocation
is rejected, as are missing/extra arguments. `--help` only prints usage.
Input symlinks and special files are rejected; imports retain the library's
16 MiB cap and schema checks. Local parent directories remain operator-owned;
synchronous file operations do not have a wall-clock timeout. Returned errors
clean up unpublished temporary output; process kill or power loss may leave
a temporary file, and a published file is not rolled back if later reading fails.

The JSON summary derives per-stream record/frame/sample counts, reset reasons,
observed close markers and matching latest snapshots from the input. Empty,
single-direction and snapshot-only evidence are supported. A replaced stream
without its own observed close marker or matching closed snapshot has unknown
final state. Active snapshot fields are independent observations; a missing
queue entry is never reconstructed as silence or called a lost PCM block solely
from a sequence difference. The source tag is an unauthenticated declaration,
including `ObservedNative`. No report certifies physical completeness or alignment.
Closed snapshots must leave enough successful attempts and frames to account
for the records actually present; contradictory loss totals are rejected.

Timing belongs to each native stream. A native batch is observed once before
being split into reference blocks; split sample timestamps use nominal-rate
offsets, not independent device measurements. A backward callback or sample
anchor marks a new continuity segment and makes that entire batch's reading
unknown without deleting its PCM or rewinding its frame cursor.

Replay also reports `ClockDiscontinuity` when adjacent PCM in the same stream
changes clock quality or callback/sample timestamp availability, or either
timestamp moves backwards. Marker records are not clock measurements. This
preserves old evidence and its original samples; it tells consumers to discard
stale timing assumptions, not that a physical clock failure or PCM gap was proven.
Equal callback timestamps, negative sample origins, and changing queue-delay or
host-observation values alone do not establish clock discontinuity or drift.

There is no selected AEC consumer. Reference blocks, speech transport blocks,
VAD windows and resampler chunks have separate sizes and purposes. Claiming the
reference reader does not insert AEC into the converted speech path. Alignment,
drift estimation and algorithm framing require confirmed consumer and device
contracts; see the [clock and consumer contract](../../../docs/refactor/audio-reference-clock-design.md#r02-时钟缺口与-consumer-契约2026-10-14).

The Mac offline combination tests also send generated PCM through the production
capture helper, final voice/cue mixer callback and one `EvidenceSession`, then
export, import and replay it. They cover exact cue cancellation, explicit
hold/resume, observation cancellation/drop, overflow, a lost close marker and
stream reopening. They open no native stream; their assertions cover software
samples, ownership and cleanup, not wall-clock or acoustic latency.

An explicit library caller still owns any real `EvidenceSession`, its bounded
pumping, finish/cancellation and export; no default recorder or runtime daemon
is enabled. The 512-record / 16 MiB limits bound software retention, not a
guaranteed number of seconds. Agree real-audio destination, retention and deletion
before authorized recording. Always inspect termination, discarded records and
footer information; an ended observation is not a complete physical stream.

Native submission receipts and exported software evidence do not prove physical
presentation, silence, echo cancellation, or full-duplex acoustic quality.
Hardware AEC and device measurement still require field validation. Existing
server shadow/default policies are unchanged.
