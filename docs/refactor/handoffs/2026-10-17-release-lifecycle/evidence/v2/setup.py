from pathlib import Path
import datetime,hashlib,json,subprocess,tarfile,time
r=Path(__file__).resolve().parent
root=r.parent.parent
e=r/'evidence'
record={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'cache_mode':'APFS clone of previous task cache; this run writes only its private clone','commands':[]}
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
archive=r/'native/sherpa-onnx-v1.13.1-linux-aarch64-static-lib.tar.bz2'
expected='86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749'
assert hashlib.sha256(archive.read_bytes()).hexdigest()==expected
libraries=[]
with tarfile.open(archive,'r:bz2') as tar:
 for member in tar.getmembers():
  if member.isfile() and member.name.endswith('.a'):
   actual=r/'cache/target/sherpa-onnx-prebuilt'/member.name
   digest=hashlib.sha256(tar.extractfile(member).read()).hexdigest()
   assert actual.is_file() and hashlib.sha256(actual.read_bytes()).hexdigest()==digest, str(actual)
   libraries.append({'member':member.name,'sha256':digest,'bytes':actual.stat().st_size})
assert len(libraries)==15
(e/'native-inputs-before.json').write_text(json.dumps({'archive':str(archive.relative_to(r)),'archive_sha256':expected,'libraries':libraries},indent=2)+'\n')
old=[]
for name in ['alia-server','alia-embedded']:
 path=r/'cache/target/release'/name
 old.append({'name':name,'sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'bytes':path.stat().st_size})
(e/'cached-release-before.json').write_text(json.dumps(old,indent=2)+'\n')
label='release-lifecycle-v2-20261017'
network=run(['docker','network','create','--internal','--label','alia.validation='+label,'alia-release-lifecycle-v2-20261017-net'])
record['network_id']=network;save()
pg=run(['docker','run','-d','--pull=never','--name','alia-release-lifecycle-v2-20261017-db','--label','alia.validation='+label,'--network',network,'-e','POSTGRES_USER=alia_test','-e','POSTGRES_PASSWORD=alia_test','-e','POSTGRES_DB=alia_release_lifecycle_v2_20261017',pgimage])
record['database_id']=pg;save()
ready=False
ready_deadline=time.monotonic()+30
while time.monotonic()<ready_deadline:
 p=subprocess.run(['docker','exec',pg,'psql','-h','127.0.0.1','-U','alia_test','-d','alia_release_lifecycle_v2_20261017','-v','ON_ERROR_STOP=1','-Atc','SELECT 1'],capture_output=True,text=True,timeout=min(5,ready_deadline-time.monotonic()))
 record.setdefault('database_readiness',[]).append({'exit_code':p.returncode,'stdout':p.stdout,'stderr':p.stderr});save()
 if p.returncode==0 and p.stdout.strip()=='1':ready=True;break
 time.sleep(min(1,max(0,ready_deadline-time.monotonic())))
assert ready and time.monotonic()<ready_deadline,'task PostgreSQL did not become TCP-ready within 30 seconds'
query='SELECT current_database(),current_user,version();'
record['database_preflight']=run(['docker','exec',pg,'psql','-U','alia_test','-d','alia_release_lifecycle_v2_20261017','-v','ON_ERROR_STOP=1','-Atc',query]);save()
argv=['docker','run','-d','--pull=never','--platform','linux/arm64','--name','alia-release-lifecycle-v2-20261017','--label','alia.validation='+label,'--network','container:'+pg,'--workdir','/workspace']
for key,value in {'CARGO_HOME':'/cache/cargo','CARGO_TARGET_DIR':'/cache/target','CARGO_BUILD_JOBS':'2','CARGO_TERM_COLOR':'never','SHERPA_ONNX_ARCHIVE_DIR':'/native'}.items():argv+=['-e',key+'='+value]
for source,target,ro in [('cache','/cache',False),('registry','/cache/cargo/registry',False),('source','/workspace',True),('native','/native',True),('evidence','/evidence',False),('runtime','/runtime',False),('artifacts','/artifacts',True)]:
 argv+=['--mount',f'type=bind,src={r/source},dst={target}'+(',readonly' if ro else '')]
argv +=[image,'sleep','infinity']
linux=run(argv);record['linux_id']=linux;save()
print(json.dumps({'linux_id':linux,'database_id':pg,'network_id':network,'source_files':len(json.loads((e/'source-manifest.json').read_text())['files']),'native_libraries_verified':len(libraries)}))
