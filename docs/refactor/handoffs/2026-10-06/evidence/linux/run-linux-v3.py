from pathlib import Path
import subprocess, json
base=Path(__file__).resolve().parent
stage=base/'linux-arm64-v3/evidence/run-stage.py'
commands=[
 ('environment', ['python3','-c', '''import subprocess,json,hashlib
from pathlib import Path
commands=[['rustc','-Vv'],['uname','-a'],['protoc','--version'],['pkg-config','--modversion','alsa','dbus-1','openssl'],['ldd','--version'],['cargo','tree','--locked','--offline','-e','features','-i','sherpa-onnx']]
records=[]
for command in commands:
 r=subprocess.run(command,capture_output=True,text=True);records.append(dict(command=command,exit_code=r.returncode,stdout=r.stdout,stderr=r.stderr));r.check_returncode()
Path('/evidence/environment.json').write_text(json.dumps(records,indent=2)+'\\n')
print(json.dumps(records))
''']),
 ('platform-libs',['cargo','test','-p','alia-core','-p','alia-proto','-p','alia-embedded','--lib','--locked','--offline','--no-fail-fast']),
 ('server-targets',['cargo','test','-p','alia-server','--lib','--test','appraisal_provenance','--test','channel_registry','--test','delivery_service','--test','delivery_text_service','--test','duplex_playback','--test','body_delivery','--test','output_control','--test','output_lifecycle','--test','server_runtime','--test','plugin_storage','--locked','--offline','--no-fail-fast']),
 ('release',['cargo','build','-p','alia-server','-p','alia-embedded','--release','--locked','--offline']),
 ('no-default',['cargo','check','-p','alia-server','-p','alia-embedded','--no-default-features','--all-targets','--locked','--offline']),
 ('binaries',['python3','-c','''import json,subprocess,hashlib
from pathlib import Path
records=[]
for name in ['alia-server','alia-embedded']:
 p=Path('/cache/target/release')/name
 records.append(dict(name=name,bytes=p.stat().st_size,sha256=hashlib.sha256(p.read_bytes()).hexdigest(),file=subprocess.check_output(['file',str(p)],text=True),ldd=subprocess.check_output(['ldd',str(p)],text=True)))
Path('/evidence/binaries.json').write_text(json.dumps(records,indent=2)+'\\n')
print(json.dumps(records))
''']),
]
for name, command in commands:
 subprocess.run(['python3',str(stage),name,*command],check=True)
pg=['docker','exec','alia-cleanup-20261006-db','psql','-U','alia_test','-d','alia_cleanup_20261006','-v','ON_ERROR_STOP=1','-c']
r=subprocess.run(pg+['CREATE SCHEMA linux_release_smoke_v3'],capture_output=True,text=True);r.check_returncode()
try:
 subprocess.run(['python3',str(stage),'release-smoke','python3','/evidence/release-smoke.py'],check=True)
finally:
 r=subprocess.run(pg+['DROP SCHEMA linux_release_smoke_v3 CASCADE'],capture_output=True,text=True)
 (base/'linux-arm64-v3/evidence/smoke-schema-cleanup.json').write_text(json.dumps({'exit_code':r.returncode,'stdout':r.stdout,'stderr':r.stderr},indent=2)+'\n');r.check_returncode()
