from pathlib import Path
import hashlib, json, shutil
r=Path(__file__).resolve().parent
root=r.parents[1]
h=root/'docs/refactor/handoffs/2026-10-11-classifier'
e=h/'evidence'
assert not e.exists(), 'archive is immutable'
allowed={'.json','.log','.txt','.py','.rs','.sha256','.patch'}
def copy(origin,dest):
 dest.parent.mkdir(parents=True,exist_ok=True)
 shutil.copyfile(origin,dest)
 assert hashlib.sha256(origin.read_bytes()).digest()==hashlib.sha256(dest.read_bytes()).digest()
for p in r.iterdir():
 if p.is_file() and p.suffix in allowed:
  dest=e/('tools' if p.suffix=='.py' else 'root')/p.name
  copy(p,dest)
for l in sorted(r.glob('linux-arm64-*')):
 for p in (l/'evidence').rglob('*'):
  if p.is_file() and p.suffix in allowed:
   copy(p,e/l.name/p.relative_to(l/'evidence'))
v=json.loads((r/'commit-verification.json').read_text())
c=json.loads((r/'resource-closure.json').read_text())
assert v['input_count']==v['matching_git_blobs'] and not v['mismatches']
assert c['all_task_containers_stopped'] and c['other_database_connections']==0 and not c['remaining_test_schemas']
files=[dict(path=str(p.relative_to(h)),bytes=p.stat().st_size,sha256=hashlib.sha256(p.read_bytes()).hexdigest()) for p in sorted(e.rglob('*')) if p.is_file()]
(h/'evidence-index.json').write_text(json.dumps(dict(source_commit=v['source_commit'],manifest_sha256=v['manifest_sha256'],scope='Final same-source classifier/catalog/cache validation; V1 was static review only, with findings and fixes preserved',files=files),indent=2)+'\n')
print(json.dumps(dict(evidence_files=len(files),source_commit=v['source_commit'],manifest_sha256=v['manifest_sha256'])))
