"""Run official WAV samples through the real local HTTP endpoint, never a fake."""

import argparse
import base64
import json
from pathlib import Path
import platform
import time
import urllib.error
import urllib.request
import wave


def pcm16(path: Path):
    import numpy as np
    import torch
    from torchaudio.functional import resample

    with wave.open(str(path), "rb") as wav:
        rate, channels, width = wav.getframerate(), wav.getnchannels(), wav.getsampwidth()
        if width != 2:
            raise ValueError(f"{path.name}: expected PCM16 WAV")
        samples = np.frombuffer(wav.readframes(wav.getnframes()), dtype="<i2").astype("float32")
    samples = samples.reshape(-1, channels).mean(axis=1) / 32768.0
    if rate != 16000:
        samples = resample(torch.from_numpy(samples), rate, 16000).numpy()
    pcm = np.clip(np.rint(samples * 32768), -32768, 32767).astype("<i2").tobytes()
    return pcm, {"original_sample_rate_hz": rate, "original_channels": channels,
                 "resampled": rate != 16000, "duration_ms": len(pcm) / 32}


def main():
    parser = argparse.ArgumentParser(description="Real local HTTP inference of official Easy-Turn WAV samples")
    parser.add_argument("--sample-dir", required=True, type=Path)
    parser.add_argument("--url", default="http://127.0.0.1:8091")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--timeout", type=float, default=180.0)
    args = parser.parse_args()
    paths = sorted(args.sample_dir.glob("*.wav"))
    if not paths:
        parser.error("sample directory contains no WAV files")
    with urllib.request.urlopen(args.url + "/health", timeout=5) as response:
        health = json.load(response)
    if health.get("model", {}).get("source") != "ASLP-lab/Easy-Turn":
        parser.error("endpoint is not serving the real Easy-Turn model")
    report = {"platform": platform.platform(), "machine": platform.machine(),
              "health": health, "samples": [], "scope": "official sample HTTP smoke, not deployment acceptance"}
    failures = 0
    for index, path in enumerate(paths):
        pcm, metadata = pcm16(path)
        body = {"request_id": f"official-smoke-{index}", "generation_epoch": index,
                "audio": {"encoding": "pcm_s16le", "sample_rate_hz": 16000,
                          "channels": 1, "data_base64": base64.b64encode(pcm).decode()}}
        request = urllib.request.Request(args.url + "/v1/detect", data=json.dumps(body).encode(),
                                          headers={"Content-Type": "application/json"})
        started = time.perf_counter()
        try:
            with urllib.request.urlopen(request, timeout=args.timeout) as response:
                status, result = response.status, json.load(response)
        except urllib.error.HTTPError as exc:
            status, result = exc.code, json.loads(exc.read())
        expected = path.name.split("_")[0]
        matched = status == 200 and result.get("state") == expected
        failures += not matched
        record = {"file": path.name, **metadata, "http_status": status,
                  "http_ms": (time.perf_counter() - started) * 1000, "expected_state": expected,
                  "state_matches_filename": matched, "result": result}
        report["samples"].append(record)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps(record, ensure_ascii=False), flush=True)
        if status in (429, 504):
            raise SystemExit("Model still running; stopped rather than enqueue more smoke requests")
    raise SystemExit(1 if failures else 0)


if __name__ == "__main__":
    main()
