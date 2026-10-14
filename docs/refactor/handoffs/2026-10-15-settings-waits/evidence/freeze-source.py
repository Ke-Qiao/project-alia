from pathlib import Path
import datetime, hashlib, json, subprocess, sys
root=Path(subprocess.check_output(['git','rev-parse','--show-toplevel'],cwd=Path(__file__).resolve().parent,text=True).strip())
output=Path(__file__).resolve().parent/sys.argv[1]
assert not output.exists(), 'source manifests are immutable'
listed=subprocess.check_output(['git','ls-files','-co','--exclude-standard','-z'],cwd=root).decode().split('\0')
files=[]
for name in sorted(set(listed)):
 if not name or name.startswith(('docs/','run/','.codex/','.agents/')):continue
 p=root/name
 if p.is_file():files.append({'path':name,'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'bytes':p.stat().st_size})
manifest={'created_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'base_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),'scope':'tracked and non-ignored untracked build/source inputs, excluding top-level docs/run/private agent metadata','files':files}
output.write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps({'path':str(output),'files':len(files),'sha256':hashlib.sha256(output.read_bytes()).hexdigest()}))
