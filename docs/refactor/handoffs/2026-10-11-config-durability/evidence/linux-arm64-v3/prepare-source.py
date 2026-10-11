#!/usr/bin/env python3
"""Copy only a Root-frozen manifest; never construct or refresh the freeze."""
from pathlib import Path
import hashlib
import json
import shutil
import sys

RUN = Path(__file__).resolve().parent.parent
ROOT = RUN.parents[2]
EVIDENCE = RUN / 'evidence'
SOURCE = RUN / 'source'

if len(sys.argv) != 2:
    raise SystemExit('Usage: prepare-source.py ROOT_FROZEN_MANIFEST')
manifest_path = Path(sys.argv[1]).resolve()
manifest_bytes = manifest_path.read_bytes()
manifest = json.loads(manifest_bytes)
if (EVIDENCE / 'source-manifest.json').exists() or any(SOURCE.iterdir()):
    raise SystemExit('Existing snapshot is immutable; use a new run directory.')
paths = set()
for item in manifest['files']:
    relative = Path(item['path'])
    if relative.is_absolute() or '..' in relative.parts or str(relative) in paths:
        raise SystemExit(f'Invalid or duplicate manifest path: {relative}')
    paths.add(str(relative))
    origin = ROOT / relative
    if not origin.is_file() or hashlib.sha256(origin.read_bytes()).hexdigest() != item['sha256']:
        raise SystemExit(f'Source no longer matches Root freeze: {relative}')
for item in manifest['files']:
    relative = Path(item['path'])
    destination = SOURCE / relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ROOT / relative, destination)
    if hashlib.sha256(destination.read_bytes()).hexdigest() != item['sha256']:
        raise SystemExit(f'Source changed during copy: {relative}')
(EVIDENCE / 'source-manifest.json').write_bytes(manifest_bytes)
(EVIDENCE / 'source.sha256').write_text(''.join(f"{item['sha256']}  {item['path']}\n" for item in manifest['files']))
print(json.dumps({'files': len(paths), 'manifest_sha256': hashlib.sha256(manifest_bytes).hexdigest()}))
