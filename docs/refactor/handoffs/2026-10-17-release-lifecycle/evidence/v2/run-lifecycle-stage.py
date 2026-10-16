#!/usr/bin/env python3
"""One bounded Linux stage; preserve failed results and immutable source evidence."""
from pathlib import Path
import datetime, hashlib, json, re, subprocess, sys, time
RUN = Path(__file__).resolve().parent
EVIDENCE, SOURCE = RUN / 'evidence', RUN / 'source'
SETUP = json.loads((EVIDENCE / 'setup.json').read_text())
CONTAINER = SETUP['linux_id']
def timestamp():
 return datetime.datetime.now(datetime.timezone.utc).isoformat()
def verify():
 manifest = json.loads((EVIDENCE / 'source-manifest.json').read_text())
 expected = {item['path']: item['sha256'] for item in manifest['files']}
 binaries = json.loads((EVIDENCE/'inputs.json').read_text())['artifacts']
 for item in binaries:
  artifact=RUN/'artifacts'/item['name']
  assert hashlib.sha256(artifact.read_bytes()).hexdigest()==item['sha256'], 'release artifact drift'
 actual = {str(path.relative_to(SOURCE)): hashlib.sha256(path.read_bytes()).hexdigest() for path in SOURCE.rglob('*') if path.is_file()}
 return {'file_count':len(expected), 'errors':sorted(path for path in expected.keys() | actual.keys() if expected.get(path)!=actual.get(path))}
if len(sys.argv)<3:
 raise SystemExit('Usage: run-stage.py NAME COMMAND [ARG...]')
name, command = sys.argv[1], sys.argv[2:]
if not re.fullmatch('[a-z0-9][a-z0-9-]*',name) or any((EVIDENCE/f'{name}.{suffix}').exists() for suffix in ['command.json','result.json','log']):
 raise SystemExit('Invalid or existing stage; choose a new name.')
record = {'started':timestamp(), 'command':command, 'container':CONTAINER,
 'environment':{'ALIA_TEST_DATABASE_URL':'isolated task PostgreSQL', 'DATABASE_URL':'unset','RUST_TEST_THREADS':'unset','RUST_MIN_STACK':'unset','DOCS_RS':'unset','SHERPA_ONNX_LIB_DIR':'unset'},
 'manifest_sha256':hashlib.sha256((EVIDENCE/'source-manifest.json').read_bytes()).hexdigest(),
 'container_watchdog_seconds':1800,'container_kill_after_seconds':10,'host_watchdog_seconds':1850,'exit_code':None}
argv = ['docker','exec',CONTAINER,'env','-u','DATABASE_URL','-u','RUST_MIN_STACK','-u','RUST_TEST_THREADS','-u','RUST_BACKTRACE','-u','DOCS_RS','-u','SHERPA_ONNX_LIB_DIR','ALIA_TEST_DATABASE_URL=postgresql://alia_test:alia_test@127.0.0.1:5432/alia_release_lifecycle_v2_20261017','timeout','--signal=TERM','--kill-after=10s','1800',*command]
started=time.monotonic()
try:
 record['source_before']=verify()
 if record['source_before']['errors']: raise RuntimeError('Frozen source mismatch before command.')
 (EVIDENCE/f'{name}.command.json').write_text(json.dumps(record,indent=2)+'\n')
 with (EVIDENCE/f'{name}.log').open('x') as log:
  proc=subprocess.Popen(argv,stdout=log,stderr=subprocess.STDOUT)
  try: record['exit_code']=proc.wait(timeout=1850)
  except subprocess.TimeoutExpired:
   record['host_timeout']=True
   try:
    killed=subprocess.run(['docker','kill',CONTAINER],capture_output=True,text=True,timeout=15)
    record['emergency_cleanup']={'exit_code':killed.returncode,'stdout':killed.stdout,'stderr':killed.stderr}
   except Exception as error: record['emergency_cleanup']={'unresolved':repr(error)}
   finally:
    proc.kill()
    proc.wait(timeout=10)
   record['exit_code']=124
except Exception as error:
 record['runner_error']=repr(error)
finally:
 try: record['source_after']=verify();record['source_drift']=record['source_after']['errors']
 except Exception as error: record['source_drift']=['verification failed: '+repr(error)]
 record.update(elapsed_seconds=round(time.monotonic()-started,3),finished_at=timestamp())
 (EVIDENCE/f'{name}.result.json').write_text(json.dumps(record,indent=2)+'\n')
 print(json.dumps({'stage':name,'exit_code':record['exit_code'],'elapsed_seconds':record['elapsed_seconds'],'source_drift':record['source_drift'],'runner_error':record.get('runner_error')}),flush=True)
raise SystemExit(record['exit_code'] or int(bool(record.get('runner_error') or record['source_drift'])))
