from pathlib import Path
import datetime, hashlib, json, shutil
r=Path(__file__).resolve().parent
e=r/'evidence'
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
assert not (e/'inputs.json').exists()
for stage in ['release-default','release-link']:
 result=json.loads((e/(stage+'.result.json')).read_text())
 assert result['exit_code']==0 and not result['source_drift']
audit=json.loads((e/'release-audit.json').read_text());assert audit['passed']
items=[]
for binary in audit['binaries']:
 name=binary['name']; source=r/'cache/target/release'/name; target=r/'artifacts'/name
 assert not target.exists() and sha(source)==binary['sha256']
 shutil.copy2(source,target);target.chmod(0o755)
 assert sha(target)==binary['sha256'] and target.stat().st_size==binary['bytes']
 items.append({'name':name,'source':str(source),'sha256':sha(target),'bytes':target.stat().st_size})
manifest=json.loads((e/'source-manifest.json').read_text())
prior=json.loads((r.parent/'source-manifest.json').read_text())
new={x['path']:x['sha256'] for x in manifest['files']};old={x['path']:x['sha256'] for x in prior['files']}
delta=sorted(k for k in new.keys()|old.keys() if new.get(k)!=old.get(k))
assert delta==['packages/alia-server/src/channels/embedded/channel.rs','packages/alia-server/src/channels/embedded/channel_lifecycle_tests.rs']
report={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'base_head':manifest['base_head'],'source_files':len(new),'source_manifest_sha256':sha(e/'source-manifest.json'),'changed_inputs_from_v1':delta,'artifacts':items}
(e/'inputs.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report))
