from pathlib import Path
import subprocess,os
r=Path(__file__).resolve().parent
os.environ['ALIA_VALIDATION_MANIFEST']='source-v2-manifest.json'
for name,cmd in [
('no-default-v2',['cargo','check','-p','alia-server','--no-default-features','--all-targets','--locked','--offline']),
('availability-no-default-v2',['cargo','test','-p','alia-server','--no-default-features','--lib','availability_tests','--locked','--offline']),
('workspace-v2',['cargo','test','--workspace','--locked','--offline','--no-fail-fast'])]:
 subprocess.run(['python3',str(r/'run_stage.py'),name,*cmd],check=True)
