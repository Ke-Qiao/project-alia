from pathlib import Path
import subprocess
base=Path(__file__).resolve().parent
stage=base/'linux-arm64-v4/evidence/run-stage.py'
commands=[
 ('environment',['python3','-c', '''import subprocess,json,hashlib
from pathlib import Path
commands=[['rustc','-Vv'],['uname','-sm'],['protoc','--version'],['pkg-config','--modversion','alsa','dbus-1','openssl'],['ldd','--version']]
records=[]
for command in commands:
 r=subprocess.run(command,capture_output=True,text=True);r.check_returncode();records.append(dict(command=command,exit_code=r.returncode,stdout=r.stdout,stderr=r.stderr))
Path('/evidence/environment.json').write_text(json.dumps(records,indent=2)+'\\n')
print(json.dumps(records))
''']),
 ('server-tests',['cargo','test','-p','alia-server','--locked','--offline','--no-fail-fast']),
 ('server-no-default',['cargo','check','-p','alia-server','--no-default-features','--all-targets','--locked','--offline']),
 ('server-release',['cargo','build','-p','alia-server','--release','--locked','--offline']),
 ('server-binary',['python3','-c','''import json,subprocess,hashlib
from pathlib import Path
p=Path('/cache/target/release/alia-server')
r=dict(name=p.name,bytes=p.stat().st_size,sha256=hashlib.sha256(p.read_bytes()).hexdigest(),file=subprocess.check_output(['file',str(p)],text=True),ldd=subprocess.check_output(['ldd',str(p)],text=True))
Path('/evidence/server-binary.json').write_text(json.dumps(r,indent=2)+'\\n');print(json.dumps(r))
''']),
]
for name,command in commands[2:]:
 subprocess.run(['python3',str(stage),name,*command],check=True)
