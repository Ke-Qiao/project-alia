from pathlib import Path
import datetime,json,subprocess
r=Path(__file__).resolve().parent
for name in ['alia-l10-linux-20261010-v1','alia-l10-linux-20261010-v2','alia-l10-20261010-db']:
 state=json.loads(subprocess.check_output(['docker','inspect',name],text=True))[0]['State']
 assert not state['Running'],name
cmd=['docker','run','-d','--pull=never','--name','alia-l11-20261010-db','-e','POSTGRES_USER=alia_test','-e','POSTGRES_PASSWORD=alia_test','-e','POSTGRES_DB=alia_l11_20261010','-p','127.0.0.1::5432','sha256:306e00ca129081f4a69081c09ff0bdf0ef5169779019200a5c22ce9e0c6148f3']
cid=subprocess.check_output(cmd,text=True).strip()
published=subprocess.check_output(['docker','port',cid,'5432/tcp'],text=True).strip()
assert published.startswith('127.0.0.1:') and not published.endswith(':5433')
record=dict(command=cmd,container_id=cid,published=published,created_at=datetime.datetime.now(datetime.timezone.utc).isoformat())
(r/'database-create.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record))
