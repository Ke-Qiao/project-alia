from pathlib import Path
import datetime, hashlib, json, re, shutil

out = Path(__file__).resolve().parent
root = out.parent.parent
record = json.loads((out / 'final-workspace.json').read_text())
assert record['exit_code'] == 0 and record['source_drift'] == []
log = (out / 'final-workspace.log').read_text()
headers = list(re.finditer(r'^\s*(Running |Doc-tests )([^\n]+)', log, flags=re.M))
targets = []
ignored = []
for i, header in enumerate(headers):
    block = log[header.end():headers[i+1].start() if i+1 < len(headers) else len(log)]
    matches = re.findall(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out', block)
    assert matches, header.group(2)
    passed, failed, skipped, measured, filtered = map(int, matches[-1])
    targets.append(dict(kind=header.group(1).strip(), target=header.group(2), passed=passed, failed=failed, ignored=skipped, measured=measured, filtered=filtered))
    ignored += re.findall(r'^test (.+?) \.\.\. ignored(?:, (.*))?$', block, flags=re.M)
totals = {key: sum(t[key] for t in targets) for key in ['passed', 'failed', 'ignored', 'measured', 'filtered']}
assert totals['failed'] == totals['filtered'] == totals['measured'] == 0
assert totals['ignored'] == len(ignored) == 5
paired = [t for t in targets if t['target'].startswith('tests/duplex_playback.rs ')]
assert len(paired) == 1 and paired[0]['passed'] == 10
binary_path = re.search(r'\(([^)]+)\)', paired[0]['target']).group(1)
binary = root / binary_path
digest = hashlib.sha256(binary.read_bytes()).hexdigest()
retained = out / 'final-duplex-playback'
assert not retained.exists()
shutil.copy2(binary, retained)
assert hashlib.sha256(retained.read_bytes()).hexdigest() == digest
binary_record = dict(at=datetime.datetime.now(datetime.timezone.utc).isoformat(), original_path=binary_path, retained_path=str(retained.relative_to(root)), sha256=digest, bytes=binary.stat().st_size, captured_after='final-workspace exit 0', source_manifest='final-source-manifest.json', build_context='Same --workspace default feature selection as baseline-build; test source changed, not the original baseline binary')
(out / 'final-binary.json').write_text(json.dumps(binary_record, indent=2)+'\n')
manifest = out / 'final-source-manifest.json'
summary = dict(at=datetime.datetime.now(datetime.timezone.utc).isoformat(), source_manifest=manifest.name, source_manifest_sha256=hashlib.sha256(manifest.read_bytes()).hexdigest(), inputs=len(json.loads(manifest.read_text())['files']), platform='macOS 26.6.2 ARM64 / Rust 1.95.0', run=record, targets=targets, totals=totals, ignored=ignored, paired=paired[0], paired_execution_batches=3, historical_timeout_reproduced=False, v01_status='open', counting='Last summary per Cargo Running/Doc-tests target; child process reentry and the two baseline repetitions not added to unique total', not_run=['Linux application/Pi tests', 'real PTY and Easy-Turn ignored cases', 'real capture/playback/actuators', 'release deployment and long-duration load'], limits=['Original V3 failing binary absent from bounded candidates', 'Current source and load cannot recreate historical conditions', 'Failure diagnostics cover inner 25-second future only; no claim of complete initialization/outer timeout/cleanup phase evidence'])
(out / 'validation-summary.json').write_text(json.dumps(summary, indent=2)+'\n')
print(json.dumps(dict(totals=totals, running_targets=sum(t['kind']=='Running' for t in targets), doc_targets=sum(t['kind']=='Doc-tests' for t in targets), final_binary_sha256=digest)))
