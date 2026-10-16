from pathlib import Path
import hashlib,json,subprocess
expected=json.loads(Path('/evidence/inputs.json').read_text())
prior=json.loads(Path('/evidence/previous-release-audit.json').read_text())
report={'commands':[],'artifacts':[],'dynamic_libraries':[]}
for argv in [['uname','-a'],['cat','/etc/os-release'],['python3','--version'],['curl','--version'],['protoc','--version']]:
 p=subprocess.run(argv,capture_output=True,text=True,timeout=15);assert p.returncode==0,p.stderr
 report['commands'].append({'command':argv,'stdout':p.stdout,'stderr':p.stderr,'exit_code':p.returncode})
for item in expected['artifacts']:
 p=Path('/artifacts')/item['name'];assert p.stat().st_size==item['bytes'] and hashlib.sha256(p.read_bytes()).hexdigest()==item['sha256']
 loader=subprocess.run(['/lib/ld-linux-aarch64.so.1','--verify',str(p)],capture_output=True,text=True,timeout=15);assert loader.returncode==0
 report['artifacts'].append({'name':p.name,'sha256':item['sha256'],'bytes':p.stat().st_size,'loader_exit_code':loader.returncode})
for item in {x['path']:x for b in prior['binaries'] for x in b['resolved_libraries']}.values():
 p=Path(item['path']);assert p.stat().st_size==item['bytes'] and hashlib.sha256(p.read_bytes()).hexdigest()==item['sha256']
 report['dynamic_libraries'].append(item)
print(json.dumps(report,indent=2))
