from pathlib import Path
import datetime,hashlib,json,subprocess
out=Path(__file__).resolve().parent
root=out.parents[1]
manifest=out/'final-source-manifest.json'
items=json.loads(manifest.read_text())['files']
commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
refs=''.join(commit+':'+x['path']+'\n' for x in items).encode()
data=subprocess.check_output(['git','cat-file','--batch'],input=refs,cwd=root)
pos=0;mismatches=[]
for item in items:
 end=data.index(b'\n',pos);header=data[pos:end].decode().split();pos=end+1
 if len(header)!=3 or header[1]!='blob':raise RuntimeError(header)
 size=int(header[2]);blob=data[pos:pos+size];pos+=size+1
 if hashlib.sha256(blob).hexdigest()!=item['sha256']:mismatches.append(item['path'])
record=dict(recorded_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),source_commit=commit,final_manifest_sha256=hashlib.sha256(manifest.read_bytes()).hexdigest(),git_blobs_checked=len(items),mismatches=mismatches,method='git cat-file --batch; SHA256 of committed blob bytes compared with final frozen inputs')
(out/'commit-verification.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record));assert not mismatches
