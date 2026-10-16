from pathlib import Path
import datetime,hashlib,json,shutil
r=Path(__file__).resolve().parent
root=r.parent.parent
destination=root/'docs/refactor/handoffs/2026-10-17-linux-release'
e=destination/'evidence'
assert not e.exists()
assert json.loads((r/'evidence/cleanup.json').read_text())['complete']
assert json.loads((r/'evidence/workspace-counts.json').read_text())['assessment']=='PASSED_RECORDED_RUN'
assert json.loads((r/'evidence/release-audit.json').read_text())['passed']
for name in ['server-no-default','embedded-no-default','release-default']:
 result=json.loads((r/f'evidence/{name}.result.json').read_text())
 assert result['exit_code']==0 and not result['source_drift']
items=[]
paths=[p for p in r.iterdir() if p.is_file() and p.suffix in ['.py','.json','.txt','.log']]+[p for p in (r/'evidence').rglob('*') if p.is_file() and p.suffix in ['.py','.json','.txt','.log']]
for path in sorted(paths):
 target=e/path.relative_to(r)
 target.parent.mkdir(parents=True,exist_ok=True)
 shutil.copy2(path,target)
 digest=hashlib.sha256(path.read_bytes()).hexdigest()
 assert hashlib.sha256(target.read_bytes()).hexdigest()==digest
 items.append({'path':str(target.relative_to(destination)),'sha256':digest,'bytes':target.stat().st_size})
record={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'base_commit':json.loads((r/'source-manifest.json').read_text())['base_head'],'source_commit':'4e2968f18d765f326e76d24a3bcdf5bba9034efc','files':items,'scope':'Byte-exact run originals, with original directory layout. Binary/cache/source snapshot remain under ignored run; binary identities are in release-audit.json. Scripts require their original run environment and are not standalone deployment tools.'}
(destination/'evidence-index.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps({'archived_files':len(items),'bytes':sum(x['bytes'] for x in items)}))
