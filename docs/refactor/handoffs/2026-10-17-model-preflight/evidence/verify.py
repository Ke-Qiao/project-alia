"""Read local release evidence and current docs; never start or probe services."""
import datetime
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[2]
RUN = ROOT / "run/validation-20261017-model-preflight"
ARCHIVE = ROOT / "docs/refactor/handoffs/2026-10-17-model-preflight"
OLD = ROOT / "docs/refactor/handoffs/2026-10-17-release-lifecycle"
SOURCE = "6c62c84ee60e3a5fd387f2172ead432031907599"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT)


baseline = json.loads((RUN / "baseline-identity.json").read_text())
manifest_path = OLD / "evidence/v2/evidence/source-manifest.json"
manifest = json.loads(manifest_path.read_text())
assert sha(manifest_path.read_bytes()) == baseline["source_manifest_sha256"]
changed = []
for entry in manifest["files"]:
    current = (ROOT / entry["path"]).read_bytes()
    if sha(current) != entry["sha256"] or len(current) != entry["bytes"]:
        changed.append({"path": entry["path"], "before_sha256": entry["sha256"],
                        "after_sha256": sha(current), "after_bytes": len(current)})
assert len(manifest["files"]) == 678
assert [entry["path"] for entry in changed] == ["config/server.example.toml"]
current_config = (ROOT / "config/server.example.toml").read_bytes()
old_config = git("show", SOURCE + ":config/server.example.toml")
head_config = git("show", baseline["head"] + ":config/server.example.toml")
deleted = (b"# Optional dedicated LLM for STT transcript correction. Omit to fall back to\n"
           b"# the configured minor/major LLMs.\n")
assert old_config.count(deleted) == 1
assert head_config == old_config
assert old_config.replace(deleted, b"", 1) == current_config
assert tomllib.loads(old_config.decode()) == tomllib.loads(current_config.decode())

for entry in baseline["artifacts"]:
    data = (ROOT / entry["path"]).read_bytes()
    assert len(data) == entry["bytes"] and sha(data) == entry["sha256"]
old_index = json.loads((OLD / "evidence-index.json").read_text())
assert len(old_index["files"]) == 148
for entry in old_index["files"]:
    data = (OLD / entry["path"]).read_bytes()
    assert len(data) == entry["bytes"] and sha(data) == entry["sha256"]

modified = git("diff", "--name-only", baseline["head"]).decode().splitlines()
untracked = git("ls-files", "--others", "--exclude-standard").decode().splitlines()
paths = sorted(set(modified + untracked))
assert all(p.startswith("docs/") or p == "config/server.example.toml" for p in paths)
assert "config/server.example.toml" in paths
git("diff", "--check")

link_count = 0
broken = []
for relative in paths:
    if not relative.endswith(".md"):
        continue
    source_path = ROOT / relative
    for match in re.finditer(r"\[[^\]\n]+\]\(([^)]+)\)", source_path.read_text()):
        target = match.group(1).strip().strip("<>").split("#", 1)[0]
        if not target or re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", target):
            continue
        link_count += 1
        if not (source_path.parent / unquote(target)).exists():
            broken.append({"source": relative, "target": target})
assert not broken, broken

result = {
    "at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "base_head": baseline["head"], "tested_source": SOURCE,
    "scope": "local static checks only; no builds/tests/network/service/device actions",
    "source_manifest_sha256": baseline["source_manifest_sha256"],
    "inputs_checked": 678, "unchanged_inputs": 677, "changed_inputs": changed,
    "config_exact_two_comment_lines_removed": True,
    "config_toml_semantics_equal_to_base_and_tested_source": True,
    "rust_cargo_lock_and_other_manifest_inputs_unchanged": True,
    "artifacts": baseline["artifacts"], "previous_archive_files_checked": 148,
    "git_diff_check": "passed", "markdown_files_checked": sum(p.endswith(".md") for p in paths),
    "local_link_paths_checked": link_count, "broken_local_link_paths": broken,
    "link_scope": "local file targets in changed/new Markdown; heading anchors not validated",
    "user_model_and_deployment_confirmation": "received; model/embedding selection explicitly pending; local assets inventoried; development account and optional containers accepted",
}
(RUN / "static-verification.json").write_text(json.dumps(result, indent=2) + "\n")
for name in ("baseline-identity.json", "static-verification.json", "verify.py"):
    (ARCHIVE / "evidence" / name).write_bytes((RUN / name).read_bytes())
files = []
for name in ("baseline-identity.json", "static-verification.json", "verify.py"):
    data = (ARCHIVE / "evidence" / name).read_bytes()
    assert data == (RUN / name).read_bytes()
    files.append({"path": "evidence/" + name, "bytes": len(data), "sha256": sha(data),
                  "kind": "local static verification original"})
(ARCHIVE / "evidence-index.json").write_text(json.dumps({
    "at": result["at"], "base_commit": baseline["head"], "source_commit": SOURCE,
    "files": files, "total_bytes": sum(f["bytes"] for f in files),
    "scope": result["scope"],
}, indent=2) + "\n")
print(json.dumps({"status": "passed", "inputs": 678, "unchanged": 677,
                  "comment_only_changes": 1, "artifacts": 2, "old_evidence": 148,
                  "local_link_paths": link_count, "archived_files": len(files)}))
