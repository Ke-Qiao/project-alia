from pathlib import Path
import subprocess, json
root=Path(__file__).resolve().parents[2]
r=Path(__file__).resolve().parent
changed=subprocess.check_output(['git','diff','--name-only','HEAD'],cwd=root,text=True).splitlines()
untracked=subprocess.check_output(['git','ls-files','--others','--exclude-standard'],cwd=root,text=True).splitlines()
paths=sorted(p for p in set(changed+untracked) if p.endswith('.rs') and (root/p).is_file())
commands=[['python3','scripts/check-server-boundaries.py','--self-test'],['python3','scripts/check-server-boundaries.py'],['git','diff','--check'],['rustfmt','--edition','2024','--check','--config','skip_children=true',*paths],['python3',str(r/'test-counter.py')]]
for command in commands:
 print('RUN',command,flush=True)
 subprocess.run(command,cwd=root,check=True)
protected=json.loads((r/'policy-boundary-audit.json').read_text())
assert all((root/path).exists() for path in protected['paths'])
subprocess.run(['git','diff','--exit-code',protected['baseline'],'--',*protected['paths']],cwd=root,check=True)
print('Protected policy/protocol/dependency/platform paths unchanged.',flush=True)
