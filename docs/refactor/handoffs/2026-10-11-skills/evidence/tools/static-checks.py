from pathlib import Path
import subprocess
root=Path(__file__).resolve().parents[2]
commands=[['python3','scripts/check-server-boundaries.py','--self-test'],['python3','scripts/check-server-boundaries.py'],['git','diff','--check'],['rustfmt','--edition','2024','--check','--config','skip_children=true','packages/alia-server/src/api/skills.rs','packages/alia-server/src/modules/interaction/skills/registry.rs','packages/alia-server/src/modules/interaction/skills/registry_tests.rs','packages/alia-server/src/modules/interaction/skills/mod.rs','packages/alia-server/tests/server_runtime.rs','packages/alia-server/tests/server_runtime/skills.rs'],['python3',str(Path(__file__).with_name('test-counter.py'))]]
for command in commands:
 print('RUN',command,flush=True)
 subprocess.run(command,cwd=root,check=True)
