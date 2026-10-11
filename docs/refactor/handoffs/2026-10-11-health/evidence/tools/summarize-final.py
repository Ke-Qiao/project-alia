from pathlib import Path
import hashlib,json,subprocess
r=Path(__file__).resolve().parent
manifest=hashlib.sha256((r/'source-v2-manifest.json').read_bytes()).hexdigest()
stages={}
for name in ['workspace-v2','no-default-v2','static-v2']:
 d=json.loads((r/(name+'-result.json')).read_text())
 assert d['source_manifest_sha256']==manifest and not d['source_drift'] and d['exit_code']==0,(name,d)
 stages['macos/'+name]=d
 if name=='workspace-v2':
  subprocess.run(['python3',str(r/'count_cargo_tests.py'),str(r/(name+'.log')),'--result',str(r/(name+'-result.json')),'--json',str(r/(name+'.counts.json')),'--text',str(r/(name+'.counts.txt'))],check=True,stdout=subprocess.DEVNULL)
  d['counts']=json.loads((r/(name+'.counts.json')).read_text())['totals']
e=r/'linux-arm64-v2/evidence'
for name in ['server-tests','server-no-default','environment']:
 d=json.loads((e/(name+'.result.json')).read_text())
 assert d['manifest_sha256']==manifest and not d['source_drift'] and d['exit_code']==0,(name,d)
 stages['linux/'+name]=d
 if name.endswith('tests'):
  subprocess.run(['python3',str(r/'count_cargo_tests.py'),str(e/(name+'.log')),'--result',str(e/(name+'.result.json')),'--json',str(e/(name+'.counts.json')),'--text',str(e/(name+'.counts.txt'))],check=True,stdout=subprocess.DEVNULL)
  d['counts']=json.loads((e/(name+'.counts.json')).read_text())['totals']
(r/'final-stage-summary.json').write_text(json.dumps(dict(manifest_sha256=manifest,input_count=len(json.loads((r/'source-v2-manifest.json').read_text())['files']),stages=stages),indent=2)+'\n')
print(json.dumps({k:dict(exit_code=v['exit_code'],seconds=v['elapsed_seconds'],counts=v.get('counts')) for k,v in stages.items()},indent=2))
