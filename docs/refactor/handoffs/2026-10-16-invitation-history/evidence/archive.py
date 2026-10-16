from pathlib import Path
import datetime,hashlib,json,shutil
out=Path(__file__).resolve().parent
root=out.parent.parent
destination=root/'docs/refactor/handoffs/2026-10-16-invitation-history'
evidence=destination/'evidence'
assert not evidence.exists()
assert json.loads((out/'final-workspace.json').read_text())['exit_code']==0
assert json.loads((out/'cleanup.json').read_text())['task_absent']
verified=json.loads((out/'commit-verification.json').read_text())
assert not verified['mismatches'] and not verified['extra_inputs'] and not verified['missing_inputs']
evidence.mkdir()
items=[]
for path in sorted(out.iterdir()):
    if not path.is_file() or path.suffix not in ('.py','.json','.log','.txt'):
        continue
    target=evidence/path.name
    shutil.copy2(path,target)
    digest=hashlib.sha256(path.read_bytes()).hexdigest()
    assert hashlib.sha256(target.read_bytes()).hexdigest()==digest
    items.append(dict(path=str(target.relative_to(destination)),sha256=digest,bytes=target.stat().st_size))
record=dict(at=datetime.datetime.now(datetime.timezone.utc).isoformat(),source_commit=verified['source_commit'],files=items,scope='Byte-exact originals; executable test binaries are retained under ignored run, identities are recorded in JSON; scripts keep their original run layout and are not standalone tools here')
(destination/'evidence-index.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(dict(archived_files=len(items),bytes=sum(x['bytes'] for x in items),source_commit=record['source_commit'])))
