from pathlib import Path
import datetime,hashlib,json
r=Path(__file__).resolve().parent
e=r/'evidence'
before=json.loads((e/'native-inputs-before.json').read_text())
errors=[]
archive=r/before['archive']
if hashlib.sha256(archive.read_bytes()).hexdigest()!=before['archive_sha256']:errors.append(str(archive))
for lib in before['libraries']:
 p=r/'cache/target/sherpa-onnx-prebuilt'/lib['member']
 if p.stat().st_size!=lib['bytes'] or hashlib.sha256(p.read_bytes()).hexdigest()!=lib['sha256']:errors.append(str(p))
record={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'archive_sha256':before['archive_sha256'],'static_libraries_checked':len(before['libraries']),'errors':errors}
assert not (e/'native-inputs-after.json').exists()
(e/'native-inputs-after.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record));assert not errors
