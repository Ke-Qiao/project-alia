# Alia Easy-Turn service

This component loads one official Easy-Turn model on the macOS **Server host**,
then serves bounded audio-window inference over loopback HTTP. It is not a Pi
service, a complete duplex pipeline, a streaming ASR service, or an execution
authority. Alia's Interaction runtime decides what a turn label means.

Implementation and service-contract tests are separate from real-weight and
macOS CPU/MPS acceptance. See [the recorded validation](VALIDATION.md) for actual
results and their scope. No real inference or latency result is implied by a
passing HTTP test. Startup fails if weights are missing, mismatched, or cannot
be loaded; no mock detector or synthesized state is available through the CLI.

## Reproducible setup and explicit downloads

Python 3.10 is the validated dependency target. Create an isolated environment
from the repository root (no global pip install):

```sh
uv venv --python 3.10 services/easy-turn/.venv
uv pip install --python services/easy-turn/.venv/bin/python \
  --build-constraint services/easy-turn/build-constraints.txt \
  --require-hashes -r services/easy-turn/requirements-macos.lock
uv pip install --python services/easy-turn/.venv/bin/python --no-deps -e services/easy-turn
```

The following command **explicitly downloads approximately 4.5 GB** of model
artifacts plus the official source. It does not start inference. Keep these
paths in the repository's ignored `run/` directory, outside tracked source.
Existing source at another revision is rejected rather than reset.

```sh
HF_HUB_DISABLE_XET=1 services/easy-turn/.venv/bin/alia-easy-turn-prepare \
  --source-dir run/models/easy-turn-source \
  --model-dir run/models/easy-turn \
  --qwen-dir run/models/qwen2.5-0.5b-instruct
```

The prepare command verifies the official checkpoint SHA-256 and records Qwen
artifact hashes. It pins both Hugging Face downloads to commits. **The serving
command performs no downloads** and checks the artifacts before loading.
`HF_HUB_DISABLE_XET=1` selects the official resumable HTTP download path, which
completed on the validation host after Xet stalled. It does not change sources
or checksum checks. All artifact caches default under the model directory.

```sh
services/easy-turn/.venv/bin/alia-easy-turn \
  --source-dir run/models/easy-turn-source \
  --model-dir run/models/easy-turn \
  --qwen-dir run/models/qwen2.5-0.5b-instruct \
  --device cpu --port 8091 --inference-timeout 60
```

The HTTP listener starts only after successful loading. Model initialization
requires additional transient memory for checkpoint and base-model tensors;
the checkpoint's file size is not the process's peak memory requirement.
`--host` accepts only loopback IPs. Run one process/worker. Stop with Ctrl-C or
SIGTERM; restarting reloads the model. Do not wrap it in multiple Uvicorn
workers, which would duplicate weights and defeat the model concurrency bound.

`--device auto` currently selects the verified CPU route. `--device mps` is an
explicit candidate, uses FP32, and performs a required-operator check before
model loading. **The pinned Torch 2.2.2 MPS route failed real inference** on the
validation Mac because `torch.isin` is not implemented there; it now fails
startup with that explanation. It is not a supported serving mode in this lock.
No silent CPU fallback is enabled. A future MPS dependency update requires its
own full sample and deployment acceptance. CPU results do not imply CUDA or
MPS performance.

## Official code and adaptation boundary

Verified on 2026-09-08:

| Artifact | Fixed source |
| --- | --- |
| Easy-Turn code | [ASLP-lab/Easy-Turn, fe175eaa8442c9a0834acf29f8640ee74c2aceca](https://github.com/ASLP-lab/Easy-Turn/tree/fe175eaa8442c9a0834acf29f8640ee74c2aceca) |
| Easy-Turn checkpoint | [ASLP-lab/Easy-Turn, 7d1f8b06776b575ac22e2fd4aa7913da2f4b8b98](https://huggingface.co/ASLP-lab/Easy-Turn/tree/7d1f8b06776b575ac22e2fd4aa7913da2f4b8b98) |
| Qwen base model/tokenizer | [Qwen/Qwen2.5-0.5B-Instruct, 7ae557604adf67be50417f59c2c2f167def9a775](https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct/tree/7ae557604adf67be50417f59c2c2f167def9a775) |

The checkpoint is 3,451,801,684 bytes; its official LFS SHA-256 is
`1cb535bb0ffe3ffa0ac941bae4898aeb87adef7b88bb58f665af08eded6f6070`.
The pinned Easy-Turn model `config.json` is empty; initialization uses the
official code's `Easy_Turn/examples/wenetspeech/whisper/conf/train.yaml`.
Model/source cards declare Apache-2.0; preserve their licenses when packaging.
Python dependency licenses are separate from model licensing.

We directly import the official `TransformerEncoder`, `LLMASR_Model`, its
`generate`, and `compute_log_mel_spectrogram`. The upstream CLI imports training
and unrelated model dependencies and uses CUDA autocast. This service instead:

- Constructs only the configured official model and uses the first official
  turn-detection prompt deterministically; callers cannot supply a prompt.
- Overrides the two upstream Hugging Face loader bindings only during model
  construction: FP32, built-in Qwen2, `trust_remote_code=False`,
  `local_files_only=True`, and eager attention. It restores the bindings after
  construction. No upstream file or architecture is rewritten.
- Applies `torch.load(weights_only=True)` and strict state-dict matching. An
  incomplete/incompatible checkpoint fails startup, including missing parameters
  that the upstream batch script only logs. It never serves random substitutes.
- Computes the official STFT/log-mel features on CPU, then transfers features
  and lengths to the chosen model device. Official `generate` performs decoding.
- During the one-time processor import, skips its unrelated global SoX buffer
  initializer and immediately restores that binding. The official feature
  function itself is unchanged. This raw-PCM path never uses SoX decoding or
  resampling; a system `libsox` installation is not required for it.
- Accepts only a single terminal official label. Missing, duplicate, or malformed
  labels are inference errors, never an inferred default `complete` or `wait`.
  The pinned checkpoint's observed literal `<|endoftext|>` suffix after that
  label is removed; arbitrary trailing text is still rejected.

Direct model dependencies are pinned in `pyproject.toml`; the resolved macOS
Python 3.10 environment is captured with hashes in `requirements-macos.lock`.
They are an inference
subset, not upstream's training `requirements.txt`: Torch/Torchaudio 2.2.2
replace its 2.1.0; PEFT 0.13.2 and Transformers 4.44.2 are paired instead of the
upstream 0.17.0/4.44.0 combination. This selection needs import and real-weight
validation; package resolution by itself is not compatibility evidence.

## HTTP v1

`GET /health` returns `status: "ready"`, `model_loaded: true`, and `model` after
successful startup. `GET /v1/capabilities` reports format/window/concurrency
limits, model/source/Qwen revisions, effective device/dtype and timeout.

`POST /v1/detect`, `Content-Type: application/json`:

```json
{
  "request_id": "detector-request-17",
  "generation_epoch": 42,
  "audio": {
    "encoding": "pcm_s16le",
    "sample_rate_hz": 16000,
    "channels": 1,
    "data_base64": "<base64 of raw little-endian signed PCM16>"
  }
}
```

Audio must contain 100 ms–30 s of complete 16 kHz mono PCM16 samples; no WAV
header. The request body is bounded to 1,284,096 bytes, including chunked uploads.
`request_id` is 1–128 characters from `[A-Za-z0-9._:-]`; `generation_epoch` is an
unsigned 64-bit integer. Unknown fields and coercion from string/float/bool are
rejected. Audio is request-local and never written to disk or logged.

```json
{
  "request_id": "detector-request-17",
  "generation_epoch": 42,
  "transcript": "请等一下",
  "state": "wait",
  "model": {
    "source": "ASLP-lab/Easy-Turn",
    "revision": "7d1f8b06776b575ac22e2fd4aa7913da2f4b8b98",
    "source_revision": "fe175eaa8442c9a0834acf29f8640ee74c2aceca",
    "qwen_revision": "7ae557604adf67be50417f59c2c2f167def9a775",
    "device": "cpu",
    "dtype": "float32"
  },
  "inference_ms": 123.4
}
```

This is an illustrative response, not a measured/model-generated sample.
States: `complete`, `incomplete`, `backchannel`, `wait`. No confidence score is
invented; the official decoder does not provide a calibrated confidence.

Errors use `{request_id?, generation_epoch?, error: {code, message, retryable}}`.
Correlation fields are echoed only when individually valid. Audio, exception
strings, filesystem paths and private model output are excluded from errors.

| HTTP | Code | Meaning |
| --- | --- | --- |
| 422 | `invalid_request` / `invalid_audio` | Invalid JSON schema, PCM, sample count or format |
| 413 | `audio_too_large` | Body or audio window too large |
| 429 | `busy` | One inference is already running; no model request queue |
| 500 | `inference_failed` | Model execution or terminal-label validation failed |
| 504 | `inference_timeout` | HTTP deadline elapsed; model work may still be running |

The service is stateless across windows and does not decide if an epoch is
current. The Server must discard stale/cancelled results and manage retries.
Timeout/disconnect does not kill native model execution: its concurrency slot
stays occupied until it actually returns. A stuck native call requires process
restart; do not launch overlapping inference on the same model after timeout.
The HTTP server additionally bounds concurrent connections to 32; Uvicorn's
own saturation response may be HTTP 503 without the application error envelope.

Accepted inference logs use Uvicorn's existing INFO logger: `accepted` records
the validated request ID/epoch, UTC Unix milliseconds, PCM byte count and audio
duration; `native_started` adds the thread-dispatch delay, and `native_finished`
records actual native elapsed time and the validated turn state or exception
class. No audio, transcript, prompt, exception message or credentials are logged.
These events do not enable third-party verbose logging or HTTP access logs.
`response_timeout` records the service's own response deadline (normally 60 s),
not the Rust caller's separate 5 s deadline or proof of a client disconnect.
Native completion may therefore be logged after the caller has already stopped
waiting; it is not evidence that the caller received or accepted that result.

## Verification

Contract tests (injected fake, no weights or GPU required):

```sh
services/easy-turn/.venv/bin/python -m unittest discover -s services/easy-turn/tests -v
```

They verify format/size/IDs, chunked-body bounds, error privacy, output-label
parsing, metadata-only inference logs, concurrent overload, and preservation of
the model slot and late native completion logging after timeout.
They **do not** verify neural inference, classification accuracy or duplex UX.

For real acceptance, explicitly prepare and start the model, send every class
from the official sample set and representative deployment recordings through
this same endpoint, and record transcript/state, effective device, runtime
versions, warm/cold latency, memory and audio-window duration. Check CPU first,
then MPS separately. Report exact failures; do not mark loading or inference as
passed if only the HTTP double ran. Retain original model audio consent/privacy
constraints when collecting deployment samples.

The following smoke runner submits the official source's WAVs through HTTP and
writes actual responses/timings incrementally. It explicitly resamples the
24/48 kHz samples to 16 kHz and mixes stereo to mono, recording these changes;
the inference service itself never resamples. Sample labels are inferred from
the official filenames solely for this smoke check, not used for predictions.

```sh
services/easy-turn/.venv/bin/python -m alia_easy_turn.smoke \
  --sample-dir run/models/easy-turn-source/samples \
  --output run/easy-turn-cpu-smoke.json
```

Any HTTP failure or mismatch causes a nonzero exit. An inference timeout stops
the run rather than accumulating retries behind a still-running native call.
