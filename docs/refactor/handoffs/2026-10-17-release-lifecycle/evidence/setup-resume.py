from pathlib import Path
import datetime,json,subprocess
r=Path(__file__).resolve().parent;e=r/'evidence'
old=json.loads((e/'setup.json').read_text());assert 'linux_id' not in old
assert not (e/'setup-completed.json').exists()
record={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'original_setup':'setup.json','reason':'initial Unix-socket readiness observed entrypoint temporary server; require actual TCP SQL readiness','images':old['images'],'baseline_containers':old['baseline_containers'],'network_id':old['network_id'],'database_id':old['database_id'],'commands':[]}
def save():(e/'setup-completed.json').write_text(json.dumps(record,indent=2)+'\n')
def run(argv):
 p=subprocess.run(argv,capture_output=True,text=True,timeout=30)
 record['commands'].append({'command':argv,'exit_code':p.returncode,'stdout':p.stdout,'stderr':p.stderr});save()
 if p.returncode:raise RuntimeError(p.stderr)
 return p.stdout.strip()
pg=old['database_id'];network=old['network_id'];image=old['images'][0]['Id']
assert run(['docker','inspect','--format','{{.Id}} {{.State.Running}} {{index .Config.Labels "alia.validation"}}',pg])==pg+' true release-lifecycle-20261017'
assert run(['docker','network','inspect','--format','{{.Id}} {{.Internal}} {{index .Labels "alia.validation"}}',network])==network+' true release-lifecycle-20261017'
record['database_tcp_preflight']=run(['docker','exec','-e','PGPASSWORD=alia_test',pg,'psql','-h','127.0.0.1','-U','alia_test','-d','alia_release_lifecycle_20261017','-v','ON_ERROR_STOP=1','-Atc','SELECT current_database(),current_user,inet_server_addr(),version();']);save()
argv=['docker','run','-d','--pull=never','--platform','linux/arm64','--name','alia-release-lifecycle-20261017','--label','alia.validation=release-lifecycle-20261017','--network','container:'+pg,'--workdir','/workspace']
for source,target,ro in [('source','/workspace',True),('artifacts','/artifacts',True),('evidence','/evidence',False),('runtime','/runtime',False)]:argv+=['--mount',f'type=bind,src={r/source},dst={target}'+(',readonly' if ro else '')]
argv +=[image,'sleep','infinity'];record['linux_id']=run(argv);save();print(json.dumps({k:record[k] for k in ['linux_id','database_id','network_id']}))
