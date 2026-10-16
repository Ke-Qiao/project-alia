from pathlib import Path
import datetime,hashlib,json,subprocess,tarfile,time
r=Path(__file__).resolve().parent
root=r.parent.parent
e=r/'evidence'
record={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'scope':'current immutable release binaries, internal network, task-only state; no Cargo or hardware','commands':[]}
def save(): (e/'setup.json').write_text(json.dumps(record,indent=2)+'\n')
def run(argv,timeout=60):
 p=subprocess.run(argv,capture_output=True,text=True,timeout=timeout)
 record['commands'].append({'command':argv,'exit_code':p.returncode,'stdout':p.stdout,'stderr':p.stderr});save()
 if p.returncode: raise RuntimeError(p.stderr)
 return p.stdout.strip()
image='sha256:6cd7a7a06976f8651cdb57fcde457e9dc7e2abe20587900dd750e1f373a25c70'
pgimage='sha256:306e00ca129081f4a69081c09ff0bdf0ef5169779019200a5c22ce9e0c6148f3'
for tag in [image,pgimage]:
 inspected=json.loads(subprocess.check_output(['docker','image','inspect',tag],text=True))[0]
 assert inspected['Id']==tag and inspected['Architecture']=='arm64' and inspected['Os']=='linux'
 record.setdefault('images',[]).append({k:inspected.get(k) for k in ['Id','Architecture','Os','RepoDigests','Created','RootFS']});save()
record['baseline_containers']=run(['docker','ps','-a','--format','{{.ID}} {{.Names}} {{.Status}} {{.Image}}'])
label='release-lifecycle-20261017'
network=run(['docker','network','create','--internal','--label','alia.validation='+label,'alia-release-lifecycle-20261017-net'])
record['network_id']=network;save()
pg=run(['docker','run','-d','--pull=never','--name','alia-release-lifecycle-20261017-db','--label','alia.validation='+label,'--network',network,'-e','POSTGRES_USER=alia_test','-e','POSTGRES_PASSWORD=alia_test','-e','POSTGRES_DB=alia_release_lifecycle_20261017',pgimage])
record['database_id']=pg;save()
ready=False
for _ in range(30):
 p=subprocess.run(['docker','exec',pg,'pg_isready','-U','alia_test','-d','alia_release_lifecycle_20261017'],capture_output=True,text=True,timeout=5)
 if p.returncode==0:ready=True;break
 time.sleep(1)
assert ready,'task PostgreSQL did not become ready'
query='SELECT current_database(),current_user,version();'
record['database_preflight']=run(['docker','exec',pg,'psql','-U','alia_test','-d','alia_release_lifecycle_20261017','-v','ON_ERROR_STOP=1','-Atc',query]);save()
argv=['docker','run','-d','--pull=never','--platform','linux/arm64','--name','alia-release-lifecycle-20261017','--label','alia.validation='+label,'--network','container:'+pg,'--workdir','/workspace']
for source,target,ro in [('source','/workspace',True),('artifacts','/artifacts',True),('evidence','/evidence',False),('runtime','/runtime',False)]:
 argv+=['--mount',f'type=bind,src={r/source},dst={target}'+(',readonly' if ro else '')]
argv +=[image,'sleep','infinity']
linux=run(argv);record['linux_id']=linux;save()
print(json.dumps({'linux_id':linux,'database_id':pg,'network_id':network,'source_files':677}))
