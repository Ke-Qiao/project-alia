from pathlib import Path
import datetime,json,subprocess,sys
r=Path(__file__).resolve().parent
e=r/'evidence'
setup=json.loads((e/'setup.json').read_text())
out=e/(sys.argv[1]+'.json')
assert not out.exists()
sql="SELECT json_build_object('schemas',(SELECT count(*) FROM pg_namespace WHERE nspname <> 'public' AND nspname <> 'information_schema' AND nspname NOT LIKE 'pg_%'),'other_clients',(SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND backend_type='client backend'));"
argv=['docker','exec',setup['database_id'],'psql','-U','alia_test','-d','alia_release_lifecycle_v2_20261017','-v','ON_ERROR_STOP=1','-Atc',sql]
p=subprocess.run(argv,capture_output=True,text=True,timeout=30)
record={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'command':argv,'exit_code':p.returncode,'stdout':p.stdout,'stderr':p.stderr}
if p.returncode==0:record.update(json.loads(p.stdout))
result=e/('lifecycle-round-'+sys.argv[1].split('-')[2]+'.result.json')
if result.exists():
 finished=datetime.datetime.fromisoformat(json.loads(result.read_text())['finished_at'])
 record['seconds_after_round_result']=round((datetime.datetime.fromisoformat(record['at'])-finished).total_seconds(),3)
out.write_text(json.dumps(record,indent=2)+'\n')
top=subprocess.run(['docker','top',setup['linux_id'],'-eo','pid,ppid,args'],capture_output=True,text=True,timeout=15)
record['processes']={'exit_code':top.returncode,'stdout':top.stdout,'stderr':top.stderr}
out.write_text(json.dumps(record,indent=2)+'\n')
assert top.returncode==0 and len(top.stdout.strip().splitlines())==2 and 'sleep infinity' in top.stdout
print(json.dumps(record))
assert p.returncode==0 and record['schemas']==record['other_clients']==0
