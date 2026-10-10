from pathlib import Path
import hashlib, json, shutil, subprocess
root = Path(__file__).resolve().parents[2]
r = Path(__file__).resolve().parent
h = root / 'docs/refactor/handoffs/2026-10-10-l11'
e = h / 'evidence'
for name in ['macos', 'linux', 'tools', 'static', 'candidates']:
    (e / name).mkdir(parents=True, exist_ok=True)
v = json.loads((r / 'commit-verification.json').read_text())
sha = v['manifest_sha256']
assert v['input_count'] == v['matching_git_blobs'] and not v['mismatches']
for stage in ['workspace-v4', 'pty-v4', 'no-default-v4', 'availability-no-default-v4', 'admission-no-default-v4', 'static-v4', 'response-failure-v4']:
    result = json.loads((r / (stage + '-result.json')).read_text())
    assert result['exit_code'] == 0 and not result['source_drift'], stage
    assert result['source_manifest_sha256'] == sha
    for suffix in ['.log', '-result.json']:
        shutil.copyfile(r / (stage + suffix), e / 'macos' / (stage + suffix))
    if stage in ['workspace-v4', 'pty-v4', 'availability-no-default-v4', 'admission-no-default-v4', 'response-failure-v4']:
        subprocess.run(['python3', str(r / 'count_cargo_tests.py'), str(e / 'macos' / (stage + '.log')), '--result', str(e / 'macos' / (stage + '-result.json')), '--json', str(e / 'macos' / (stage + '.counts.json')), '--text', str(e / 'macos' / (stage + '.counts.txt'))], stdout=subprocess.DEVNULL, check=True)
for name in ['macos-environment.json', 'macos-native-archive.json']:
    shutil.copyfile(r / name, e / 'macos' / name)
l = r / 'linux-arm64-v4/evidence'
for stage in ['environment', 'server-tests-repeat', 'server-no-default', 'server-release', 'server-binary']:
    result = json.loads((l / (stage + '.result.json')).read_text())
    assert result['exit_code'] == 0 and not result['source_drift'], stage
    assert result['manifest_sha256'] == sha
for p in l.iterdir():
    if p.is_file():
        shutil.copyfile(p, e / 'linux' / p.name)
subprocess.run(['python3', str(r / 'count_cargo_tests.py'), str(e / 'linux/server-tests-repeat.log'), '--result', str(e / 'linux/server-tests-repeat.result.json'), '--json', str(e / 'linux/server-tests-repeat.counts.json'), '--text', str(e / 'linux/server-tests-repeat.counts.txt')], stdout=subprocess.DEVNULL, check=True)
for name in ['source-v4-manifest.json', 'commit-verification.json', 'database-create.json', 'resource-closure.json']:
    shutil.copyfile(r / name, e / name)
for p in r.glob('*.py'):
    shutil.copyfile(p, e / 'tools' / p.name)
for name in ['source-v1-manifest.json', 'source-v2-manifest.json', 'v1-v2-difference.json', 'v2-v3-difference.json', 'static-v2.log', 'static-v2-result.json', 'integration-check.log', 'integration-check-repaired.log']:
    shutil.copyfile(r / name, e / 'candidates' / name)
for name in ['source-manifest.json', 'source.sha256', 'container-create.json']:
    shutil.copyfile(r / 'linux-arm64-v2/evidence' / name, e / 'candidates' / ('linux-v2-' + name))

for p in r.iterdir():
    if p.is_file() and (('-v3' in p.name and p.suffix in ['.json', '.log', '.txt']) or p.name == 'v3-v4-difference.json'):
        shutil.copyfile(p, e / 'candidates' / p.name)
(e / 'candidates/linux-v3').mkdir(exist_ok=True)
for p in (r / 'linux-arm64-v3/evidence').iterdir():
    if p.is_file():
        shutil.copyfile(p, e / 'candidates/linux-v3' / p.name)
for origin, name in [(root / 'run/l11-response-20261010/static-audit.json', 'response.json'), (root / 'run/l11-companion-20261010/static-audit.json', 'companion.json'), (r / 'context-static-audit.json', 'context.json'), (r / 'counter-self-test.json', 'counter-self-test.json')]:
    shutil.copyfile(origin, e / 'static' / name)
closure = json.loads((e / 'resource-closure.json').read_text())
assert not closure['remaining_test_schemas'] and closure['other_database_connections'] == 0 and closure['all_task_containers_stopped']
files = [dict(path=str(p.relative_to(h)), bytes=p.stat().st_size, sha256=hashlib.sha256(p.read_bytes()).hexdigest()) for p in sorted(e.rglob('*')) if p.is_file()]
(h / 'evidence-index.json').write_text(json.dumps(dict(source_commit=v['source_commit'], manifest_sha256=sha, scope='Final V4 same-source validation; preserved static-review correction and integration compile failures; task fixtures only', files=files), indent=2) + '\n')
print(json.dumps(dict(evidence_files=len(files), source_commit=v['source_commit'], manifest_sha256=sha)))
