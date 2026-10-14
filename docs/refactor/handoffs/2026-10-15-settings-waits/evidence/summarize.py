from pathlib import Path
import datetime, hashlib, json, re
out = Path(__file__).resolve().parent
name = 'green-server'
record = json.loads((out / (name + '.json')).read_text())
assert record['exit_code'] == 0 and record['source_drift'] == []
targets = []
ignored = []
for block in re.split(r'^\s*Running ', (out / (name + '.log')).read_text(), flags=re.M)[1:]:
    matches = re.findall(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out', block)
    if matches:
        values = matches[-1]
        targets.append(dict(target=block.splitlines()[0], passed=int(values[0]), failed=int(values[1]), ignored=int(values[2]), filtered=int(values[4])))
    ignored += re.findall(r'^test (.+?) \.\.\. ignored(?:, (.*))?$', block, flags=re.M)
totals = {key: sum(t[key] for t in targets) for key in ['passed', 'failed', 'ignored', 'filtered']}
assert totals['failed'] == 0 and totals['filtered'] == 0
manifest = out / 'green-source-manifest.json'
summary = dict(at=datetime.datetime.now(datetime.timezone.utc).isoformat(), source_manifest=manifest.name, final_source_manifest='final-source-manifest.json', format_transition='format-only-transition.json', source_manifest_sha256=hashlib.sha256(manifest.read_bytes()).hexdigest(), inputs=len(json.loads(manifest.read_text())['files']), platform='macOS 26.6.2 ARM64 / Rust 1.95.0', run=record, targets=targets, totals=totals, ignored=ignored, counting='Last summary per Cargo Running target; child process reentry and targeted/repeated runs not added', not_run=['Linux application tests', 'Embedded and Pi tests', 'real PTY and Easy-Turn ignored cases', 'hardware/real capture or playback/actuators', 'release deployment and long-duration load'], limits=['No universal Settings deadline; synchronous file IO cannot be preempted', 'Local HTTP/1 and mock providers only', 'No distinct HTTP gate-queued GET/PATCH stage witness'])
(out / 'validation-summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print(json.dumps(totals))
