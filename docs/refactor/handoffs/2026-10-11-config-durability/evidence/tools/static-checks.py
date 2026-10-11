from pathlib import Path
import subprocess,json
root=Path(__file__).resolve().parents[2]
r=Path(__file__).resolve().parent
paths=['packages/alia-server/src/config.rs','packages/alia-server/src/config/publication.rs','packages/alia-server/src/config/publication/testing.rs','packages/alia-server/src/config/publication/tests.rs','packages/alia-server/src/oobe/bootstrap.rs','packages/alia-server/src/oobe/routes.rs','packages/alia-server/src/oobe/routes/tests.rs','packages/alia-server/src/oobe/routes/tests/durability.rs','packages/alia-server/src/bootstrap/setup.rs','packages/alia-server/tests/duplex_playback.rs']
commands=[['python3','scripts/check-server-boundaries.py','--self-test'],['python3','scripts/check-server-boundaries.py'],['git','diff','--check'],['rustfmt','--edition','2024','--check','--config','skip_children=true',*paths],['python3',str(r/'test-counter.py')]]
for command in commands:
 print('RUN',command,flush=True)
 subprocess.run(command,cwd=root,check=True)
protected=json.loads((r/'policy-boundary-audit.json').read_text())
subprocess.run(['git','diff','--exit-code',protected['baseline'],'--',*protected['paths']],cwd=root,check=True)
print('Policy/protocol/permission/dependency protected paths unchanged.',flush=True)
