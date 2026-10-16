from pathlib import Path
import hashlib,json,re,subprocess
root=Path('/cache/target/release')
e=Path('/evidence')
previous={x['name']:x for x in json.loads((e/'cached-release-before.json').read_text())}
report={'binaries':[],'commands':[]}
def command(argv):
 p=subprocess.run(argv,capture_output=True,text=True,timeout=60)
 report['commands'].append({'command':argv,'exit_code':p.returncode,'stdout':p.stdout,'stderr':p.stderr})
 if p.returncode: raise RuntimeError(str(argv)+p.stderr)
 return p.stdout
try:
 for name in ['alia-server','alia-embedded']:
  path=root/name
  item={'name':name,'bytes':path.stat().st_size,'mtime_ns':path.stat().st_mtime_ns,'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}
  report['binaries'].append(item)
  item['changed_from_cached_release'] = item['sha256']!=previous[name]['sha256']
  if name=='alia-server': assert item['changed_from_cached_release'],'stale Server release artifact'
  item['file']=command(['file',str(path)])
  assert 'ARM aarch64' in item['file'] and 'ELF 64-bit' in item['file']
  item['elf_header']=command(['readelf','-h',str(path)])
  assert 'AArch64' in item['elf_header']
  item['program_headers']=command(['readelf','-l',str(path)])
  assert '/lib/ld-linux-aarch64.so.1' in item['program_headers']
  item['dynamic']=command(['readelf','-d',str(path)])
  assert 'RPATH' not in item['dynamic'] and 'RUNPATH' not in item['dynamic']
  item['ldd']=command(['ldd',str(path)])
  assert 'not found' not in item['ldd']
  command(['/lib/ld-linux-aarch64.so.1','--verify',str(path)])
  item['loader_list']=command(['/lib/ld-linux-aarch64.so.1','--list',str(path)])
  item['resolved_libraries']=[]
  for library in sorted(set(re.findall(r'(/[^\s()]+)',item['ldd']))):
   lib=Path(library)
   item['resolved_libraries'].append({'path':str(lib.resolve()),'sha256':hashlib.sha256(lib.read_bytes()).hexdigest(),'bytes':lib.stat().st_size})
  if name=='alia-embedded':
   symbols=subprocess.run(['nm','--defined-only',str(path)],capture_output=True,text=True,timeout=60)
   assert symbols.returncode==0,symbols.stderr
   item['sherpa_defined_symbols']=[line for line in symbols.stdout.splitlines() if re.search(r' \w SherpaOnnx\w+',line)]
   assert len(item['sherpa_defined_symbols'])>0,'no defined Sherpa C API symbols'
   item['safe_help']=command([str(path),'--help'])
   assert 'Usage: alia-embedded' in item['safe_help']
   assert not list(Path('/runtime').iterdir()),'help created runtime files'
 report['passed']=True
finally:
 (e/'release-audit.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({'passed':True,'binaries':[{k:x[k] for k in ['name','bytes','sha256']} for x in report['binaries']]}))
