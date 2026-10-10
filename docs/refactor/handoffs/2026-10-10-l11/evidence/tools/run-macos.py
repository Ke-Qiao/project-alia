from pathlib import Path
import subprocess,os
r=Path(__file__).resolve().parent
os.environ['ALIA_VALIDATION_MANIFEST']='source-v4-manifest.json'
for name,cmd in [
('no-default-v4',['cargo','check','-p','alia-server','--no-default-features','--all-targets','--locked','--offline']),
('availability-no-default-v4',['cargo','test','-p','alia-server','--no-default-features','--lib','availability_tests','--locked','--offline']),
('admission-no-default-v4',['cargo','test','-p','alia-server','--no-default-features','--test','server_runtime','unavailable_insights_fail_before_tts_initialization','--locked','--offline']),
('workspace-v4',['cargo','test','--workspace','--locked','--offline','--no-fail-fast'])]:
 subprocess.run(['python3',str(r/'run_stage.py'),name,*cmd],check=True)
