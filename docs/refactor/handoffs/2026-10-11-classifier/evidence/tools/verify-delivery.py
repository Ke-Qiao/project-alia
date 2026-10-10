from pathlib import Path
import hashlib,json,re,subprocess
root=Path(__file__).resolve().parents[2]
r=Path(__file__).resolve().parent
h=root/'docs/refactor/handoffs/2026-10-11-classifier'
index=json.loads((h/'evidence-index.json').read_text())
errors=[]
for e in index['files']:
 p=h/e['path']
 if not p.is_file() or p.stat().st_size!=e['bytes'] or hashlib.sha256(p.read_bytes()).hexdigest()!=e['sha256']:errors.append(e['path'])
assert not errors,errors
m=json.loads((r/'source-v2-manifest.json').read_text());sha=hashlib.sha256((r/'source-v2-manifest.json').read_bytes()).hexdigest()
assert sha==index['manifest_sha256']
head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
for e in m['files']:
 for revision in [index['source_commit'],head]:
  blob=subprocess.check_output(['git','show',f"{revision}:{e['path']}"],cwd=root)
  if hashlib.sha256(blob).hexdigest()!=e['sha256']:errors.append(f"{revision}:{e['path']}")
assert not errors,errors
files=[root/'docs/refactor'/p for p in ['classifier-2026-10-11.md','resume-checkpoint-classifier-2026-10-11.md','checklists/classifier-2026-10-11.md','checklists/layering-next.md']]+list(h.glob('*.md'))
checked_links=0
for p in files:
 for target in re.findall(r'\[[^\]]*\]\(([^)]+)\)',p.read_text()):
  target=target.split('#',1)[0]
  if not target or re.match(r'^[a-z]+:',target):continue
  checked_links+=1
  if not (p.parent/target).exists():errors.append(f'{p.relative_to(root)}:{target}')
assert not errors,errors
record={'head':head,'source_commit':index['source_commit'],'source_inputs':len(m['files']),'evidence_files':len(index['files']),'checked_links':checked_links,'errors':errors}
(r/'delivery-verification.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record))
