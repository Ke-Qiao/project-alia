from pathlib import Path
import hashlib, json, os, subprocess
r=Path(__file__).resolve().parent
old=r.parent/'validation-20261011-config-durability/linux-arm64-v1/source'
l=r/'linux-arm64-v2'
subprocess.run(['python3',str(l/'evidence/prepare-source.py'),str(r/'source-v2-manifest.json')],check=True)
preserved=0
for p in (l/'source').rglob('*'):
 if p.is_file():
  before=old/p.relative_to(l/'source')
  if before.is_file() and hashlib.sha256(before.read_bytes()).digest()==hashlib.sha256(p.read_bytes()).digest():
   os.utime(p,ns=(before.stat().st_atime_ns,before.stat().st_mtime_ns));preserved+=1
command=json.loads((r/'linux-v2-command.json').read_text())
cid=subprocess.check_output(command,text=True).strip()
(l/'evidence/container-create.json').write_text(json.dumps(dict(command=command,container_id=cid,unchanged_source_timestamps_preserved=preserved),indent=2)+'\n')
print(json.dumps(dict(container_id=cid,preserved=preserved)))
