from pathlib import Path
import datetime,json,subprocess
out=Path(__file__).resolve().parent
created=json.loads((out/'database-create.json').read_text());cid=created['container_id']
assert json.loads((out/'final-workspace.json').read_text())['exit_code']==0
resource=json.loads((out/'final-resources.json').read_text())
assert resource['schemas']==resource['other_clients']==0
assert not (out/'cleanup.json').exists()
records=[]
def run(command):
 r=subprocess.run(command,capture_output=True,text=True)
 records.append(dict(command=command,exit_code=r.returncode,stdout=r.stdout,stderr=r.stderr))
 assert r.returncode==0,records[-1]
 return r.stdout.strip()
identity=run(['docker','inspect','--format','{{.Id}} {{index .Config.Labels "alia.validation"}}',cid])
assert identity==cid+' v01-server-startup-20261015'
run(['docker','stop','--timeout','10',cid])
run(['docker','rm','-v',cid])
assert not run(['docker','ps','-a','--filter','id='+cid,'--format','{{.ID}}'])
run(['docker','ps','--format','{{.ID}} {{.Names}} {{.Image}}'])
record=dict(at=datetime.datetime.now(datetime.timezone.utc).isoformat(),removed_task_container=cid,task_absent=True,other_containers_modified=False,shared_runtime_left_running=True,records=records)
(out/'cleanup.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(dict(task_absent=True,removed_task_container=cid)))
