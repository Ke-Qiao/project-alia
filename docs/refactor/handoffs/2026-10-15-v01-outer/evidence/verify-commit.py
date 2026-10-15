from pathlib import Path
import datetime, hashlib, json, subprocess
out = Path(__file__).resolve().parent
root = Path(subprocess.check_output(['git', 'rev-parse', '--show-toplevel'], cwd=out, text=True).strip())
manifest = out / 'final-v2-source-manifest.json'
items = json.loads(manifest.read_text())['files']
commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
refs = ''.join(commit + ':' + x['path'] + '\n' for x in items).encode()
data = subprocess.check_output(['git', 'cat-file', '--batch'], input=refs, cwd=root)
pos = 0
mismatches = []
for item in items:
    end = data.index(b'\n', pos)
    header = data[pos:end].decode().split()
    pos = end + 1
    assert len(header) == 3 and header[1] == 'blob', header
    size = int(header[2])
    blob = data[pos:pos+size]
    pos += size + 1
    if hashlib.sha256(blob).hexdigest() != item['sha256']:
        mismatches.append(item['path'])
actual = set(subprocess.check_output(['git', 'ls-tree', '-r', '--name-only', commit], cwd=root, text=True).splitlines())
actual = {p for p in actual if not p.startswith(('docs/', 'run/', '.codex/', '.agents/'))}
expected = {x['path'] for x in items}
record = dict(at=datetime.datetime.now(datetime.timezone.utc).isoformat(), source_commit=commit, manifest_sha256=hashlib.sha256(manifest.read_bytes()).hexdigest(), checked_inputs=len(items), mismatches=mismatches, extra_inputs=sorted(actual-expected), missing_inputs=sorted(expected-actual))
(out / 'commit-verification.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record))
assert not mismatches and actual == expected
