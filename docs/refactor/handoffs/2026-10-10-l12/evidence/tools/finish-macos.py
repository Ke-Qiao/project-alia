from pathlib import Path
import os,subprocess
r=Path(__file__).resolve().parent
env=dict(os.environ,ALIA_VALIDATION_MANIFEST="source-v4-manifest.json")
commands=[('no-default-v4', ['cargo', 'check', '--workspace', '--no-default-features', '--all-targets', '--locked', '--offline']), ('availability-no-default-v4', ['cargo', 'test', '-p', 'alia-server', '--no-default-features', '--lib', 'availability_tests', '--locked', '--offline']), ('admission-no-default-v4', ['cargo', 'test', '-p', 'alia-server', '--no-default-features', '--test', 'server_runtime', 'unavailable_insights_fail_before_tts_initialization', '--locked', '--offline'])]
for name,args in commands:
 subprocess.run(["python3",str(r/"run_stage.py"),name,*args],env=env,check=True)
