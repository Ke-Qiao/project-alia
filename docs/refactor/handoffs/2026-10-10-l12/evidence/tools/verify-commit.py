from pathlib import Path
import hashlib,json,subprocess
r=Path(__file__).resolve().parent
m=r/'source-v4-manifest.json';data=json.loads(m.read_text())
commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
errors=[]
for item in data['files']:
 blob=subprocess.check_output(['git','show',f"{commit}:{item['path']}"])
 if hashlib.sha256(blob).hexdigest()!=item['sha256']:errors.append(item['path'])
assert not errors,errors
record=dict(source_commit=commit,manifest_sha256=hashlib.sha256(m.read_bytes()).hexdigest(),input_count=len(data['files']),matching_git_blobs=len(data['files']),mismatches=errors)
(r/'commit-verification.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
