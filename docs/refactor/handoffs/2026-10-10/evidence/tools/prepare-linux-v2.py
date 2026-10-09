from pathlib import Path
import json,shutil,subprocess,os,hashlib
r=Path(__file__).resolve().parent
old=r.parent/'validation-20261009-layering/linux-arm64-v1'
l=r/'linux-arm64-v2'
for d in ['evidence','source','runtime']:(l/d).mkdir(parents=True,exist_ok=True)
for name in ['prepare-source.py','run-stage.py']:
 (l/'evidence'/name).write_text((old/'evidence'/name).read_text().replace('l09','l10').replace('20261009','20261010').replace('alia-l10-linux-20261010-v1','alia-l10-linux-20261010-v2'))
subprocess.run(['python3',str(l/'evidence/prepare-source.py'),str(r/'source-v2-manifest.json')],check=True)
preserved=0
for p in (l/'source').rglob('*'):
 if p.is_file():
  previous=old/'source'/p.relative_to(l/'source')
  if previous.is_file() and hashlib.sha256(p.read_bytes()).digest()==hashlib.sha256(previous.read_bytes()).digest():
   os.utime(p,ns=(previous.stat().st_atime_ns,previous.stat().st_mtime_ns));preserved+=1
shutil.copyfile(old/'evidence/native-archive.json',l/'evidence/native-archive.json')
cmd=json.loads((old/'evidence/container-create.json').read_text())['command']
cmd=[x.replace('alia-l09-','alia-l10-').replace('20261009','20261010').replace('validation-20261010-layering','validation-20261010-l10') for x in cmd]
cmd=[x.replace('alia-l10-linux-20261010-v1','alia-l10-linux-20261010-v2').replace('validation-20261010-l10/linux-arm64-v1','validation-20261010-l10/linux-arm64-v2') for x in cmd]
cid=subprocess.check_output(cmd,text=True).strip()
(l/'evidence/container-create.json').write_text(json.dumps(dict(command=cmd,container_id=cid,cache='sequential reuse of stopped L09 cache',unchanged_source_timestamps_preserved=preserved),indent=2)+'\n')
print(json.dumps(dict(container_id=cid,preserved=preserved)))
