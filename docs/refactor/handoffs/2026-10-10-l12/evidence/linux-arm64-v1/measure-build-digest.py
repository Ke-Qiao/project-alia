#!/usr/bin/env python3
"""Measure production's SHA-256 loop using the test's exact compiled sha2 rlib.

Run inside the existing Linux holder; no Cargo, network, DB or package changes.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time


def file_identity(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while block := source.read(65536):
            digest.update(block)
    return {"path": str(path), "bytes": path.stat().st_size, "sha256": digest.hexdigest()}


def command(args):
    started = time.monotonic_ns()
    result = subprocess.run(args, text=True, capture_output=True, timeout=60)
    record = {
        "argv": [str(arg) for arg in args],
        "elapsed_ns": time.monotonic_ns() - started,
        "exit_code": result.returncode,
        "stdout": result.stdout,
        "stderr": result.stderr,
    }
    if result.returncode:
        raise RuntimeError(json.dumps(record))
    return record


def run(binary, output, source):
    deps = binary.parent
    profile = deps.parent
    artifact = binary.name.removesuffix(".exe")
    fingerprint = profile / ".fingerprint" / f"alia-server-{artifact.removeprefix('server_runtime-')}"
    target_json = fingerprint / "test-integration-test-server_runtime.json"
    target = json.loads(target_json.read_text())
    sha_deps = [dep for dep in target["deps"] if dep[1] == "sha2"]
    assert len(sha_deps) == 1, sha_deps
    expected_fingerprint = sha_deps[0][3]
    candidates = []
    for path in (profile / ".fingerprint").glob("sha2-*/lib-sha2"):
        if int.from_bytes(bytes.fromhex(path.read_text().strip()), "little") == expected_fingerprint:
            candidates.append(path)
    assert len(candidates) == 1, candidates
    sha_fingerprint = candidates[0]
    sha_metadata = json.loads(sha_fingerprint.with_suffix(".json").read_text())
    rlib = deps / f"lib{sha_fingerprint.parent.name}.rlib"
    assert rlib.is_file(), rlib
    output.mkdir(parents=True, exist_ok=True)
    helper = output / "build-digest-probe"
    report = {
        "purpose": "Same sha2 rlib and 64 KiB read/update loop; no inference from total build time alone",
        "binary_before": file_identity(binary),
        "sha2_rlib": file_identity(rlib),
        "sha2_fingerprint": file_identity(sha_fingerprint),
        "sha2_features": sha_metadata["features"],
        "target_fingerprint": file_identity(target_json),
        "target_sha2_dependency": sha_deps[0],
        "probe_source": file_identity(source),
        "rustc_version": command(["rustc", "--version", "--verbose"]),
    }
    report["compile"] = command([
        "rustc", "--edition=2021", "-C", "opt-level=0", "-C", "debuginfo=2",
        "--extern", f"sha2={rlib}", "-L", f"dependency={deps}", str(source), "-o", str(helper),
    ])
    report["probe_binary"] = file_identity(helper)
    vectors = output / "probe-vector.bin"
    vectors.write_bytes(bytes(range(256)) * 256 + b"x")
    report["vector_probe"] = command([str(helper), str(vectors)])
    measured_vector = json.loads(report["vector_probe"]["stdout"])
    assert measured_vector["bytes"] == vectors.stat().st_size
    assert measured_vector["sha256"] == file_identity(vectors)["sha256"]
    report["production_algorithm_probe"] = command([str(helper), str(binary)])
    measured = json.loads(report["production_algorithm_probe"]["stdout"])
    report["sha256sum"] = command(["sha256sum", str(binary)])
    report["binary_after"] = file_identity(binary)
    assert report["binary_before"] == report["binary_after"], "test binary changed during measurement"
    assert measured["bytes"] == report["binary_before"]["bytes"]
    assert measured["sha256"] == report["sha256sum"]["stdout"].split()[0]
    assert measured["sha256"] == report["binary_before"]["sha256"]
    report["digest_and_bytes_match"] = True
    (output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({
        "result": str(output / "result.json"),
        "binary": report["binary_before"],
        "sha2_rlib": report["sha2_rlib"],
        "probe": measured,
        "sha256sum_elapsed_ns": report["sha256sum"]["elapsed_ns"],
    }, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--source", type=Path, default=Path(__file__).with_name("build-digest-probe.rs"))
    args = parser.parse_args()
    run(args.binary.resolve(), args.out.resolve(), args.source.resolve())
