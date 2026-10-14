from pathlib import Path
import datetime,json,subprocess
out=Path(__file__).resolve().parent
records=[]
for cmd in [
 ['docker','ps','-a','--filter','name=alia-r03-a-20261014-db','--format','{{.ID}} {{.Names}}'],
 ['docker','inspect','--format','{{.Name}} {{json .State}}','965d0a454d15','905470a16802','c5aaca070870'],
 ['docker','ps','--format','{{.ID}} {{.Names}}'],
 ['orbctl','status'],
]:
 r=subprocess.run(cmd,text=True,capture_output=True,timeout=30)
 records.append(dict(command=cmd,exit_code=r.returncode,stdout=r.stdout,stderr=r.stderr))
assert records[0]['exit_code']==0 and records[0]['stdout'].strip()==''
record={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'initial_cleanup_script_exit':1,'initial_cleanup_failure':'cleanup.py assertion at line18 failed on third pre-existing container: astrbot is running again. Prior assertions confirm owned task container id+label, stop, stopped state and rm -v succeeded. First attempt stdout records were held in memory and not saved after assertion; this read-only follow-up is the persisted verification.','task_database_removed':True,'task_database_anonymous_volume_removal':'docker rm -v completed successfully before assertion','existing_services':'Home Assistant and old alia-postgres stopped; astrbot restarted08:48:26UTC after prior restoration, initiator unknown','runtime_policy':'Leave astrbot and OrbStack active; no further stop of unrelated workload','records':records}
(out/'cleanup.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps({'task_database_removed':True,'runtime':'preserved for running astrbot','initial_cleanup_assertion':'recorded, follow-up verified'}))
