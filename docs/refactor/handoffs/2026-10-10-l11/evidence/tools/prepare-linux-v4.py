from pathlib import Path
import json, subprocess, os, hashlib
r = Path(__file__).resolve().parent
old = r / 'linux-arm64-v3'
l = r / 'linux-arm64-v4'
for directory in ['evidence', 'source', 'runtime']:
    (l / directory).mkdir(parents=True, exist_ok=True)
for name in ['prepare-source.py', 'run-stage.py']:
    source = (old / 'evidence' / name).read_text().replace('alia-l11-linux-20261010-v3', 'alia-l11-linux-20261010-v4')
    (l / 'evidence' / name).write_text(source)
subprocess.run(['python3', str(l / 'evidence/prepare-source.py'), str(r / 'source-v4-manifest.json')], check=True)
preserved = 0
for p in (l / 'source').rglob('*'):
    if p.is_file():
        previous = old / 'source' / p.relative_to(l / 'source')
        if previous.is_file() and hashlib.sha256(p.read_bytes()).digest() == hashlib.sha256(previous.read_bytes()).digest():
            os.utime(p, ns=(previous.stat().st_atime_ns, previous.stat().st_mtime_ns))
            preserved += 1
command = json.loads((old / 'evidence/container-create.json').read_text())['command']
command = [part.replace('alia-l11-linux-20261010-v3', 'alia-l11-linux-20261010-v4').replace(str(old), str(l)) for part in command]
cid = subprocess.check_output(command, text=True).strip()
(l / 'evidence/container-create.json').write_text(json.dumps(dict(command=command, container_id=cid, unchanged_source_timestamps_preserved=preserved, cache='sequential reuse after V3 Cargo completion'), indent=2) + '\n')
print(json.dumps(dict(container_id=cid, preserved=preserved)))
