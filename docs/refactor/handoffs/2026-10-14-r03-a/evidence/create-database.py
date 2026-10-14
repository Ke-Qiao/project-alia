from pathlib import Path
import datetime,json,subprocess
out=Path(__file__).resolve().parent
assert not (out/'database-create.json').exists()
for name in ['alia-home-assistant-dev-home-assistant-1','alia-postgres','astrbot']:
 state=subprocess.check_output(['docker','inspect','--format','{{.State.Running}}',name],text=True).strip()
 assert state=='false',(name,state)
cmd=['docker','run','-d','--pull=never','--name','alia-r03-a-20261014-db','--label','alia.validation=r03-a-20261014','-e','POSTGRES_USER=alia_test','-e','POSTGRES_PASSWORD=alia_test','-e','POSTGRES_DB=alia_r03_a_20261014','-p','127.0.0.1::5432','sha256:306e00ca129081f4a69081c09ff0bdf0ef5169779019200a5c22ce9e0c6148f3']
cid=subprocess.check_output(cmd,text=True).strip()
published=subprocess.check_output(['docker','port',cid,'5432/tcp'],text=True).strip()
assert published.startswith('127.0.0.1:') and not published.endswith(':5433')
record=dict(command=cmd,container_id=cid,published=published,created_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),existing_services_restored_stopped=True)
(out/'database-create.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record))
