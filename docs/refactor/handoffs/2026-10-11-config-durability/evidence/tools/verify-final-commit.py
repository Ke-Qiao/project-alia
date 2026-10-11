from pathlib import Path
import subprocess,json,hashlib
r=Path(__file__).resolve().parent;root=r.parents[1];h=root/'docs/refactor/handoffs/2026-10-11-config-durability'
index=json.loads((h/'evidence-index.json').read_text())
head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
for entry in index['files']:
 path=str((h/entry['path']).relative_to(root))
 blob=subprocess.check_output(['git','show',f'{head}:{path}'],cwd=root)
 assert hashlib.sha256(blob).hexdigest()==entry['sha256'],path
manifest=json.loads((r/'source-v3-manifest.json').read_text())
for entry in manifest['files']:
 blob=subprocess.check_output(['git','show',f"{head}:{entry['path']}"],cwd=root)
 assert hashlib.sha256(blob).hexdigest()==entry['sha256'],entry['path']
changed=subprocess.check_output(['git','diff','--name-only',index['source_commit'],head],cwd=root,text=True).splitlines()
assert changed and all(p.startswith('docs/') for p in changed),changed
status=subprocess.check_output(['git','status','--porcelain'],cwd=root,text=True)
assert not status,status
record={'source_commit':index['source_commit'],'docs_commit':head,'input_count':len(manifest['files']),'evidence_files':len(index['files']),'all_git_blobs_verified':True,'docs_commit_contains_docs_only':True,'working_tree_clean':True}
(r/'final-delivery-record.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record))
