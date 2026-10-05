# Scoped wake cue example

Only `wakeword/` is an active built-in voice-flow example. Its `config.json`
selects a short WAV after the authenticated VoiceSession Ready. Playback uses the
original session scope and the native local-cue source at gain 0.2; it does not
close, pause, or otherwise control the input session.

`wakeword/sample.wav` reuses the repository's former `thinking/thinking.wav`
sample byte-for-byte: PCM16, mono, 16 kHz, 0.28 seconds. It is test material for scoped cue playback,
not a natural spoken wake acknowledgement or a validated production prompt. No audio was generated or downloaded.
Replace it with the desired short PCM16 WAV when configuring the device.

Copy the `wakeword` directory into the configured
`[direct_response_defaults].audio_dir`. The import writes its metadata to SQLite
and removes that copied `config.json` only after a successful import. Keep this
source example intact. Audio files remain on disk. Imports with no usable audio
reference are rejected and keep their source config for correction.

Cue records require at least one nonempty audio path with a finite positive
weight. Bundle import checks referenced files exist; scoped playback validates
PCM16 format, duration (at most five seconds), and file size (at most 4 MiB).
The `action` field is a cue lookup alias. Action-only cancel records and automatic
spoken cancel aliases are unsupported. The old thinking, listening, fallback,
error, and cancel trigger templates have been removed with their unused callers.
