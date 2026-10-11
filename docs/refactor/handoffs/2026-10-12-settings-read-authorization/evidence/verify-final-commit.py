from pathlib import Path
import hashlib,json,subprocess
r=Path(__file__).resolve().parent; root=r.parents[1]
out=root/'docs/refactor/handoffs/2026-10-12-settings-read-authorization'
index=json.loads((out/'evidence-index.json').read_text()); source=index['source_commit']
head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
assert head!=source
changed=subprocess.check_output(['git','diff','--name-only',source,head],cwd=root,text=True).splitlines()
assert changed and all(p.startswith('docs/') for p in changed),changed
for item in json.loads((r/'source-v3-manifest.json').read_text())['files']:
 blob=subprocess.check_output(['git','show',head+':'+item['path']],cwd=root)
 assert hashlib.sha256(blob).hexdigest()==item['sha256'],item['path']
for item in index['files']:
 p='docs/refactor/handoffs/2026-10-12-settings-read-authorization/'+item['path']
 blob=subprocess.check_output(['git','show',head+':'+p],cwd=root)
 assert len(blob)==item['bytes'] and hashlib.sha256(blob).hexdigest()==item['sha256'],p
clean=not subprocess.check_output(['git','status','--porcelain'],cwd=root,text=True).strip();assert clean
record={'source_commit':source,'docs_commit':head,'all_git_blobs_verified':True,'docs_commit_contains_docs_only':True,'working_tree_clean':clean}
(r/'final-delivery-record.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
