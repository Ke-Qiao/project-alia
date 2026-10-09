from pathlib import Path
import datetime,json,subprocess
r=Path(__file__).resolve().parent
pg='alia-l10-20261010-db';linux='alia-l10-linux-20261010-v1'
def sql(query):
 return subprocess.check_output(['docker','exec',pg,'psql','-U','alia_test','-d','alia_l10_20261010','-tA','-c',query],text=True).strip()
schemas=sql("SELECT nspname FROM pg_namespace WHERE nspname NOT IN ('public','information_schema') AND nspname NOT LIKE 'pg_%' ORDER BY nspname").splitlines()
connections=int(sql("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid()"))
assert not schemas,schemas
assert connections==0,connections
ids={}
for name in [linux,'alia-l10-linux-20261010-v2',pg]:
 before=json.loads(subprocess.check_output(['docker','inspect',name],text=True))[0]
 subprocess.run(['docker','stop','--time','5',name],check=True,stdout=subprocess.DEVNULL)
 after=json.loads(subprocess.check_output(['docker','inspect',name],text=True))[0]
 assert not after['State']['Running']
 ids[name]={'id':after['Id'],'image':after['Image'],'status':after['State']['Status'],'exit_code':after['State']['ExitCode'],'finished_at':after['State']['FinishedAt']}
record=dict(recorded_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),remaining_test_schemas=schemas,other_database_connections=connections,all_task_containers_stopped=True,containers=ids,user_database_5433='not accessed',cargo_jobs='all validation stages completed',cached_artifacts='retained in ignored run directory')
(r/'resource-closure.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
