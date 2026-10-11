from pathlib import Path
import hashlib,json,re,subprocess
r=Path(__file__).resolve().parent
root=r.parents[1];out=root/'docs/refactor/handoffs/2026-10-12-settings-read-authorization'
index=json.loads((out/'evidence-index.json').read_text()); errors=[]
manifest=json.loads((r/'source-v3-manifest.json').read_text())
for item in manifest['files']:
 blob=subprocess.check_output(['git','show',index['source_commit']+':'+item['path']],cwd=root)
 if hashlib.sha256(blob).hexdigest()!=item['sha256']: errors.append('source '+item['path'])
for item in index['files']:
 p=out/item['path']
 if not p.is_file() or p.stat().st_size!=item['bytes'] or hashlib.sha256(p.read_bytes()).hexdigest()!=item['sha256']:errors.append('evidence '+item['path'])
paths=list(out.glob('*.md'))+[root/p for p in ['docs/refactor/settings-read-authorization-2026-10-12.md','docs/refactor/checklists/settings-read-authorization-2026-10-12.md','docs/refactor/resume-checkpoint-settings-read-authorization-2026-10-12.md','docs/operations/memory.md','docs/operations/server-embedded.md','packages/alia-server/docs/configuration.md']]
links=0
for p in paths:
 for target in re.findall(r'\[[^\]]*\]\(([^)]+)\)',p.read_text()):
  if target.startswith(('https://','http://','#')):continue
  target=target.split('#',1)[0]
  if not (p.parent/target).exists():errors.append('link '+str(p)+' -> '+target)
  links+=1
record={'source_commit':index['source_commit'],'source_inputs':len(manifest['files']),'evidence_files':len(index['files']),'checked_links':links,'errors':errors}
(r/'delivery-verification.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record));assert not errors
