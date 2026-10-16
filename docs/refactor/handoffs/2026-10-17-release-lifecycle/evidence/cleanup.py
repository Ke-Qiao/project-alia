from pathlib import Path
import datetime,json,subprocess
r=Path(__file__).resolve().parent
e=r/'evidence'
setup=json.loads((e/'setup-completed.json').read_text())
assert not (e/'cleanup.json').exists()
record={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'commands':[],'complete':False}
def save(): (e/'cleanup.json').write_text(json.dumps(record,indent=2)+'\n')
def command(argv):
 p=subprocess.run(argv,capture_output=True,text=True,timeout=45)
 record['commands'].append({'command':argv,'exit_code':p.returncode,'stdout':p.stdout,'stderr':p.stderr});save()
 if p.returncode:raise RuntimeError(str(argv)+p.stderr)
 return p.stdout.strip()
try:
 for role in ['linux_id','database_id']:
  cid=setup[role]
  identity=command(['docker','inspect','--format','{{.Id}} {{index .Config.Labels "alia.validation"}}',cid])
  assert identity==cid+' release-lifecycle-20261017'
 network=setup['network_id']
 identity=command(['docker','network','inspect','--format','{{.Id}} {{.Internal}} {{index .Labels "alia.validation"}}',network])
 assert identity==network+' true release-lifecycle-20261017'
 top=command(['docker','top',setup['linux_id'],'-eo','pid,ppid,args'])
 assert len(top.splitlines())==2 and 'sleep infinity' in top,'unexpected task process before cleanup'
 sql="SELECT json_build_object('schemas',(SELECT count(*) FROM pg_namespace WHERE nspname <> 'public' AND nspname <> 'information_schema' AND nspname NOT LIKE 'pg_%'),'other_clients',(SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND backend_type='client backend'));"
 resources=json.loads(command(['docker','exec',setup['database_id'],'psql','-U','alia_test','-d','alia_release_lifecycle_20261017','-v','ON_ERROR_STOP=1','-Atc',sql]))
 record['database_before_cleanup']=resources;save()
 assert resources['schemas']==resources['other_clients']==0,resources
 pg=json.loads(command(['docker','inspect',setup['database_id']]))[0]
 volumes=[x['Name'] for x in pg['Mounts'] if x['Type']=='volume']
 record['database_volumes']=volumes;save()
 for role in ['linux_id','database_id']:
  cid=setup[role]
  command(['docker','stop','--timeout','10',cid])
  command(['docker','rm','-v',cid])
  assert not command(['docker','ps','-a','--filter','id='+cid,'--format','{{.ID}}'])
 command(['docker','network','rm',network])
 assert not command(['docker','network','ls','--filter','id='+network,'--format','{{.ID}}'])
 remaining=command(['docker','volume','ls','--format','{{.Name}}']).splitlines()
 assert not set(volumes)&set(remaining)
 record['remaining_running_containers']=command(['docker','ps','--format','{{.ID}} {{.Names}} {{.Image}}'])
 record['complete']=True
except Exception as error:
 record['error']=repr(error)
 raise
finally: save()
print(json.dumps({'complete':True,'database_before_cleanup':resources,'removed_task_containers':[setup[x] for x in ['linux_id','database_id']],'removed_task_network':network}))
