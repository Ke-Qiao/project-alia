from pathlib import Path
import json,hashlib,subprocess
r=Path(__file__).resolve().parent
stages={'macos_workspace':r/'workspace-v1-result.json','macos_no_default':r/'no-default-v1-result.json','linux_server':r/'linux-arm64-v1/evidence/server-tests.result.json','linux_no_default':r/'linux-arm64-v1/evidence/server-no-default.result.json','static':r/'static-final-v1-result.json'}
results={name:json.loads(path.read_text()) for name,path in stages.items()}
for name,result in results.items():
 assert result['exit_code']==0 and not result.get('source_drift'),(name,result)
counts={}
for name,log,result in [('macos_workspace',r/'workspace-v1.log',stages['macos_workspace']),('linux_server',r/'linux-arm64-v1/evidence/server-tests.log',stages['linux_server'])]:
 out=r/(name+'.counts.json')
 subprocess.run(['python3',str(r/'count_cargo_tests.py'),str(log),'--result',str(result),'--json',str(out),'--text',str(r/(name+'.counts.txt'))],check=True,stdout=subprocess.DEVNULL)
 data=json.loads(out.read_text());assert data['assessment']=='PASSED_RECORDED_RUN',data['assessment'];counts[name]=data['totals']
manifest=r/'source-v1-manifest.json'
record={'manifest_sha256':hashlib.sha256(manifest.read_bytes()).hexdigest(),'inputs':len(json.loads(manifest.read_text())['files']),'counts':counts,'stages':results}
(r/'final-stage-summary.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps({'counts':counts,'inputs':record['inputs']}))
