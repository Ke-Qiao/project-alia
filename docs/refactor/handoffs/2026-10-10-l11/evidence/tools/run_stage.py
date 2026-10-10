#!/usr/bin/env python3
"""Record one validation command against the frozen integration source."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

root = Path(__file__).resolve().parents[2]
evidence = Path(__file__).resolve().parent
manifest_name = os.environ.get("ALIA_VALIDATION_MANIFEST", "source-v4-manifest.json")
assert Path(manifest_name).name == manifest_name
manifest_path = evidence / manifest_name
manifest = json.loads(manifest_path.read_text())
name, command = sys.argv[1], sys.argv[2:]
assert name.replace("-", "").isalnum() and command
assert not (evidence / (name + "-result.json")).exists(), "stage already recorded"


def drift():
    expected = {entry["path"] for entry in manifest["files"]}
    listed = subprocess.check_output(["git", "ls-files", "-co", "--exclude-standard", "-z"], cwd=root).decode().split("\0")
    current = {p for p in listed if p and not p.startswith(("docs/", "run/", ".codex/", ".agents/")) and (root / p).is_file()}
    added = sorted(current - expected)
    return added + [entry["path"] for entry in manifest["files"]
            if not (root / entry["path"]).is_file()
            or hashlib.sha256((root / entry["path"]).read_bytes()).hexdigest()
            != entry["sha256"]]


before = drift()
if before:
    raise SystemExit("source changed before validation: " + repr(before))
env = os.environ.copy()
for key in ("RUST_MIN_STACK", "RUST_TEST_THREADS", "RUST_BACKTRACE", "DOCS_RS"):
    env.pop(key, None)
env.update({
    "CARGO_TARGET_DIR": "/Users/ke-qiao/Developer/project-alia/target",
    "CARGO_TERM_COLOR": "never",
    "CARGO_BUILD_JOBS": "2",
    "ALIA_TEST_DATABASE_URL": "postgresql://alia_test:alia_test@127.0.0.1:32769/alia_l11_20261010",
    "SHERPA_ONNX_ARCHIVE_DIR": "/Users/ke-qiao/Developer/project-alia/run/worktrees/alia-audio-clock-a33ec67/target/sherpa-onnx-prebuilt",
})
started_at = datetime.datetime.now(datetime.timezone.utc).isoformat()
started = time.monotonic()
with (evidence / (name + ".log")).open("wb") as output:
    result = subprocess.run(command, cwd=root, env=env, stdin=subprocess.DEVNULL,
                            stdout=output, stderr=subprocess.STDOUT)
after = drift()
record = {
    "command": command,
    "started_at": started_at,
    "finished_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "elapsed_seconds": round(time.monotonic() - started, 2),
    "exit_code": result.returncode,
    "source_manifest_sha256": hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
    "source_drift": after,
    "build_jobs": 2,
    "rust_min_stack": "unset",
    "rust_test_threads": "default (unset)",
    "rust_backtrace": "unset",
    "docs_rs": "unset",
    "native_archive_sha256": "ca7baf61f331ea6b848e3630e566647302c26960c7303932bc33125074871eeb",
}
(evidence / (name + "-result.json")).write_text(json.dumps(record, indent=2) + "\n")
print(json.dumps(record))
sys.exit(result.returncode or bool(after))
