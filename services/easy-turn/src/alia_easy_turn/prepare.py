"""Explicit opt-in artifact download. Never imported by service startup."""

import argparse
import json
import os
from pathlib import Path
import subprocess

from .model import sha256
from .provenance import (
    CHECKPOINT_SHA256, MODEL_ID, MODEL_REVISION, QWEN_FILES,
    QWEN_ID, QWEN_REVISION, SOURCE_REVISION, SOURCE_URL,
)


def main():
    parser = argparse.ArgumentParser(description="Explicitly download ~4.5 GB of pinned Easy-Turn/Qwen artifacts")
    parser.add_argument("--source-dir", required=True, type=Path)
    parser.add_argument("--model-dir", required=True, type=Path)
    parser.add_argument("--qwen-dir", required=True, type=Path)
    args = parser.parse_args()
    os.environ.setdefault("HF_HOME", str(args.model_dir.resolve() / ".cache/hf"))
    os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")
    from huggingface_hub import hf_hub_download, snapshot_download

    if not args.source_dir.exists():
        subprocess.run(["git", "clone", "--no-checkout", SOURCE_URL, str(args.source_dir)], check=True)
        subprocess.run(["git", "-C", str(args.source_dir), "checkout", "--detach", SOURCE_REVISION], check=True)
    revision = subprocess.check_output(
        ["git", "-C", str(args.source_dir), "rev-parse", "HEAD"], text=True
    ).strip()
    if revision != SOURCE_REVISION:
        parser.error("Existing source directory is at another revision; use a new directory")
    checkpoint = Path(hf_hub_download(
        MODEL_ID, "checkpoint.pt", revision=MODEL_REVISION, local_dir=args.model_dir,
    ))
    if sha256(checkpoint) != CHECKPOINT_SHA256:
        raise ValueError("Downloaded checkpoint SHA-256 differs from the pinned official value")
    snapshot_download(QWEN_ID, revision=QWEN_REVISION, local_dir=args.qwen_dir,
                      allow_patterns=[*QWEN_FILES, "LICENSE"])
    manifest = {"source": QWEN_ID, "revision": QWEN_REVISION,
                "sha256": {name: sha256(args.qwen_dir / name) for name in QWEN_FILES}}
    (args.qwen_dir / "alia-artifacts.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print("Pinned source and model artifacts prepared. No inference has been run.")


if __name__ == "__main__":
    main()
