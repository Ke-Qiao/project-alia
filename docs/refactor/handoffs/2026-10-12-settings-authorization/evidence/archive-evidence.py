from pathlib import Path
import hashlib, json, shutil
r=Path(__file__).resolve().parent
root=r.parents[1]
out=root/'docs/refactor/handoffs/2026-10-12-settings-authorization'
dest=out/'evidence'
assert not dest.exists(), 'evidence archive already exists'
files=[]
for p in sorted(r.rglob('*')):
 if not p.is_file() or any(part in ('source','runtime','__pycache__') for part in p.relative_to(r).parts): continue
 if p.suffix not in ('.py','.json','.log','.txt') or p.name.startswith(('final-delivery','delivery-verification')): continue
 target=dest/p.relative_to(r); target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(p,target)
 files.append({'path':str(target.relative_to(out)),'bytes':target.stat().st_size,'sha256':hashlib.sha256(target.read_bytes()).hexdigest()})
index={'source_commit':json.loads((r/'commit-verification.json').read_text())['source_commit'],'manifest_sha256':hashlib.sha256((r/'source-v2-manifest.json').read_bytes()).hexdigest(),'files':files}
(out/'evidence-index.json').write_text(json.dumps(index,indent=2)+'\n')
print(json.dumps({'evidence_files':len(files)}))
