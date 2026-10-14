from pathlib import Path
import datetime,json,subprocess
out=Path(__file__).resolve().parent
assert json.loads((out/'green-server.json').read_text())['exit_code']==0
name='alia-r03-a-20261014-db'
expected='b6c25625616e033953c6a4643d39f4d5d44fb88d1435d78900f1f26efb71d67e'
records=[]
def run(cmd):
 r=subprocess.run(cmd,text=True,capture_output=True,timeout=60)
 records.append(dict(command=cmd,exit_code=r.returncode,stdout=r.stdout,stderr=r.stderr))
 return r
r=run(['docker','inspect','--format','{{.Id}} {{index .Config.Labels "alia.validation"}}',name]);assert r.returncode==0 and r.stdout.strip()==expected+' r03-a-20261014'
r=run(['docker','stop','--time','10',name]);assert r.returncode==0
r=run(['docker','inspect','--format','{{json .State}}',name]);assert r.returncode==0 and not json.loads(r.stdout)['Running']
r=run(['docker','logs','--tail','35',name])
r=run(['docker','rm','-v',name]);assert r.returncode==0
for container in ['965d0a454d15','905470a16802','c5aaca070870']:
 r=run(['docker','inspect','--format','{{.Name}} {{.State.Running}}',container]);assert r.returncode==0 and r.stdout.strip().endswith(' false')
r=run(['docker','ps','--format','{{.ID}} {{.Names}}']);assert r.returncode==0
if not r.stdout.strip():
 r=run(['orbctl','stop']);assert r.returncode==0
 r=run(['orbctl','status']);assert r.stdout.strip()=='Stopped'
else:
 records.append({'note':'New running containers exist; left runtime active to avoid interfering with unrelated work'})
(out/'cleanup.json').write_text(json.dumps({'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'records':records},indent=2)+'\n')
print(json.dumps(records[-2:]))
