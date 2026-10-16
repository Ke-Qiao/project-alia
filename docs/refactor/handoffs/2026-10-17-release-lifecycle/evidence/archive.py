from pathlib import Path
import datetime,hashlib,json,shutil,tomllib
r=Path(__file__).resolve().parent; root=r.parent.parent
destination=root/'docs/refactor/handoffs/2026-10-17-release-lifecycle'; e=destination/'evidence'
assert not e.exists()
assert json.loads((r/'evidence/cleanup.json').read_text())['complete']
assert json.loads((r/'v2/evidence/cleanup.json').read_text())['complete']
assert json.loads((r/'v2/evidence/workspace-counts-confirmed.json').read_text())['assessment']=='PASSED_RECORDED_RUN'
source=json.loads((r/'v2/evidence/source-commit-verification.json').read_text())
assert not source['mismatches'] and not source['extra_inputs'] and not source['missing_inputs']
paths=[]
for base in (r,r/'v2'):
 paths.extend(p for p in base.iterdir() if p.is_file() and p.suffix in ['.py','.json','.txt','.log'])
 paths.extend(p for p in (base/'evidence').iterdir() if p.is_file() and p.suffix in ['.py','.json','.txt','.log','.toml','.headers'])
secrets=[]
for base in (r,r/'v2'):
 runtime=base/'runtime/staging'
 secrets.append(json.loads((runtime/'private.json').read_text())['password'].encode())
 secrets.append(tomllib.loads((runtime/'server.toml').read_text())['auth']['jwt_secret'].encode())
 for p in runtime.rglob('*.key'):
  raw=p.read_bytes()
  if len(raw)>20:secrets.append(raw)
items=[]
for path in sorted(set(paths)):
 raw=path.read_bytes();assert not any(secret in raw for secret in secrets),'private value present in '+str(path)
 target=e/path.relative_to(r);target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(path,target)
 digest=hashlib.sha256(raw).hexdigest();assert hashlib.sha256(target.read_bytes()).hexdigest()==digest
 items.append({'path':str(target.relative_to(destination)),'sha256':digest,'bytes':target.stat().st_size,'kind':'derived view' if path.name=='workspace-default.derived.log' else 'run original'})
report={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'base_commit':'1c632184946ec4ad90a3d42d3adf2508383c798d','source_commit':source['source_commit'],'files':items,'private_values_scan':'Task passwords, random JWT signing secrets and generated private-key file contents absent from selected evidence. Private runtime directories and full settings excluded.','scope':'Byte-exact copies from both failed V1 and fixed V2, including original invalid parse and explicitly derived one-heading view. Executables/cache/full snapshots remain in ignored run, with recorded identities. Scripts depend on original task environment, not standalone deployment tools.'}
(destination/'evidence-index.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({'archived_files':len(items),'bytes':sum(x['bytes'] for x in items),'source_commit':source['source_commit']}))
