# Easy-Turn component validation

These are dated component results, not Pi audio, AEC, microphone duplex
interaction, Alia end-to-end latency, or release acceptance. Keep the original
failures and each run's identity separate from later observations.

## Production scheduling correction (2026-11-02)

One controlled pair used the same fixed 3.256 s official PCM, unchanged CPU
FP32 model and 18 Torch threads, with inference on one worker thread in each
new process. The foreground process took **1,544.967 ms**; the process launched
with `taskpolicy -b` took **4,583.239 ms**. Both returned `complete` with the same
transcript SHA. Encoder and Qwen generation stages both slowed in the background
case. This supports correcting the deployment scheduling class; one pair does
not establish a percentile, SLA, or the cause of every field failure. See the
[foreground](../../run/software-recovery-20261102/easy-turn/worker-foreground.json)
and [background](../../run/software-recovery-20261102/easy-turn/worker-background.json)
stage records. The earlier October 31 temporary `taskpolicy -B` attempt showed
no clear improvement; its original evidence and rollback remain unchanged.

The only deployed change was `ProcessType` from `Background` to `Interactive`
in both `~/Library/LaunchAgents/com.alia.easy-turn.plist` and
`~/Library/Application Support/Alia/services/com.alia.easy-turn.plist`.
After stopping PID 3949 and reloading the job, PID **41472** became ready in
**4.704 s**, and the loaded launchd class was checked. Both new plist files have
SHA-256 `946ad67af9971d7962c0037b8a231088920fcaa55571151658f27519ae644bd3`.
The [activation record](../../run/software-recovery-20261102/easy-turn/scheduling-activation.json)
and [rollback plan](../../run/software-recovery-20261102/easy-turn/scheduling-plan.json)
identify the exact deployment change.

Three fixed PCM requests then passed through Alia's real Rust adapter with the
original **5,000 ms** client deadline: `complete` **1,535.027 ms**, `backchannel`
**383.876 ms**, and `wait` **436.418 ms**. The complete case was the first request
after restart, with **zero warmup requests**. Python's 60 s native response
deadline, request concurrency 1, model threads, weights, prompt and generation
parameters were unchanged. No Python service implementation, dependency,
attention implementation, precision or audio/output limit was changed in this
scheduling correction. No microphone or playback was used.

The October 31 cold sample failure remains a failure: the Rust caller exceeded
its original deadline while native work completed later at 5,960.658 ms.
The two earlier live sessions had unknown input content and lengths, so these
new component successes do not establish their complete cause. `incomplete`,
long sentences with normal breathing, human speech, mixed-load/resource bounds,
upgrade/recovery, and separate AEC acceptance remain open. See the
[bounded-run summary](../../run/software-recovery-20261102/easy-turn/summary.json).

Historical source and binary archives keep their original commit identities.
The October 25 `6123691` source archive predates the October 31 metadata
diagnostics; this November 2 host plist change is separately recorded deployment
state. An old source archive does not attest to all current host configuration.
The remaining sections below describe the **September 8 historical baseline**;
their counts and timing must not be added to this run or used to erase failures.

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
