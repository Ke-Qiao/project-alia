from pathlib import Path
import datetime,json,subprocess,sys
out=Path(__file__).resolve().parent
created=json.loads((out/'database-create.json').read_text());cid=created['container_id']
assert subprocess.check_output(['docker','inspect','--format','{{index .Config.Labels "alia.validation"}}',cid],text=True).strip()=='v01-outer-20261015'
sql="SELECT json_build_object('schemas',(SELECT count(*) FROM pg_namespace WHERE nspname <> 'public' AND nspname <> 'information_schema' AND nspname NOT LIKE 'pg_%'),'other_clients',(SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND backend_type='client backend'));"
record=json.loads(subprocess.check_output(['docker','exec',cid,'psql','-U','alia_test','-d','alia_v01_outer_20261015','-Atc',sql],text=True))
record.update(at=datetime.datetime.now(datetime.timezone.utc).isoformat(),container_id=cid)
path=out/(sys.argv[1]+'.json');assert not path.exists();path.write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
assert record['schemas']==0 and record['other_clients']==0
