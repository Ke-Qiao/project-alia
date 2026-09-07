# Easy-Turn component validation

Date: 2026-09-08. These are component results, not Pi audio, AEC, microphone
duplex interaction, Alia end-to-end latency, or release acceptance.

## Host and artifacts

- Validation host: Apple M5 Max, 64 GiB memory, macOS 26.6.2, arm64.
- Python 3.10.20; Torch/Torchaudio 2.2.2; default Torch CPU thread count 18.
- Exact runtime dependency versions and hashes: `requirements-macos.lock`.
- Easy-Turn code: `fe175eaa8442c9a0834acf29f8640ee74c2aceca`.
- Easy-Turn model: `7d1f8b06776b575ac22e2fd4aa7913da2f4b8b98`.
- Qwen: `7ae557604adf67be50417f59c2c2f167def9a775`.
- Official 3,451,801,684-byte checkpoint SHA-256 verified before inference.
- Artifacts are local under ignored `run/models/`; they are not committed.

The deployment Mac may differ from this validation host. Sandbox execution
reported MPS unavailable, but a normal-permission query reported MPS built and
available. This distinction is not evidence of a model operator failure.

## Checks completed

| Check | Actual result |
| --- | --- |
| Fixed dependency installation | Passed in isolated `services/easy-turn/.venv` |
| Hash-locked dependency consistency | 66 packages checked against the lock |
| HTTP/output contract tests | 14 passed; these inject a fake inference implementation |
| Official code import | Real model class, encoder and feature function imported |
| Official feature extraction | 1 s of 16 kHz PCM produced finite `(100, 80)` features |
| Strict CPU model initialization | All official checkpoint parameters matched; service became ready |
| CPU official-sample HTTP smoke | 16/16 HTTP 200; all four categories represented; labels matched all 16 filenames |
| MPS real-sample attempt | Failed: Torch 2.2.2 lacks `aten::isin.Tensor_Tensor_out` for MPS |
| MPS startup guard after diagnosis | Passed negative check: exits with a clear error before starting HTTP |

The dependency installation initially failed because Whisper's build imports
`pkg_resources`. Pinning build-time setuptools to 80.9.0 resolved that observed
failure. The official processor's unused SoX import initializer was isolated as
described in the README; its actual feature function was exercised afterward.

The first real HTTP run exposed a format mismatch: the model emitted a literal
`<|endoftext|>` after the turn label. The service rejected these results rather
than inventing classifications. After accepting that exact observed suffix and
adding a regression test, the complete CPU sample run passed.

## CPU measurements

One sequential pass over the 16 official repository samples through the real
loopback HTTP service, CPU FP32, default 18 Torch threads, after loading weights:

- First request model/feature processing: **1,448.90 ms**.
- Remaining 15 requests: **286.01–536.95 ms**, median **395.01 ms**.
- Samples contained 0.688–3.520 s of audio. The smoke client explicitly
  resampled the original 24/48 kHz samples and mixed the one stereo example.
- The service used a 180 s deadline during this smoke run; no inference timeout
  occurred. The normal CLI default is 60 s.

These are the service's measured feature-extraction plus inference times, not
user speech-end to answer-audio times. The first request includes first-use
work but excludes model loading. This small sample set does not estimate
deployment accuracy or a reliable production percentile.

Raw local report: `run/easy-turn-cpu-smoke.json` (not committed). Reproduce via
the README startup and `python -m alia_easy_turn.smoke` commands. No reference
transcript is provided to the model; expected labels are only compared after
receiving its actual output.

## MPS result and startup guard

The same fixed model loaded onto MPS, but all sample requests failed before
generation. Direct execution of an official sample identified Transformers
`_prepare_special_tokens` calling the unsupported MPS `torch.isin` operation.
The failed HTTP attempt is recorded in `run/easy-turn-mps-smoke.json`.

The service now probes that required operation before model loading. With this
lock, an explicit MPS request fails startup clearly rather than exposing a ready
endpoint that always returns 500. CPU fallback remains disabled, and `auto`
selects the verified CPU route. MPS dependency/operator improvements are a
separate conditional follow-up; no MPS latency or classification success is
claimed from the failed attempt.

## Remaining validation

- A new MPS dependency stack, required-operator check, and full sample execution;
  CPU results cannot certify MPS.
- Target deployment hardware/model coexistence, peak memory, sustained load,
  window cadence, noise and overlapping speakers.
- Real Pi audio transport, cancellation integration, acoustic stopping time,
  AEC and full-duplex interaction quality.
