from pathlib import Path
import datetime, hashlib, json
r=Path(__file__).resolve().parent; e=r/'evidence'; v=r/'v2/evidence'
def read(p):return json.loads(p.read_text())
assert not (r/'validation-summary.json').exists()
red=read(r/'sensor-owner-red.json'); green=read(r/'sensor-owner-green.json')
assert red['exit_code']==101 and green['exit_code']==0 and not red['source_drift'] and not green['source_drift']
old=read(e/'round-1.json');assert not old['passed'] and old['shutdown'][0]['exit_code']==1
counts=read(v/'workspace-counts-confirmed.json');assert counts['assessment']=='PASSED_RECORDED_RUN'
stages={name:read(v/(name+'.result.json')) for name in ['workspace-default','server-no-default','embedded-no-default','release-default','release-link','prepare','embedded-preflight','lifecycle-round-1','lifecycle-round-2']}
assert all(x['exit_code']==0 and not x['source_drift'] for x in stages.values())
inputs=read(v/'inputs.json');audit=read(v/'release-audit.json');assert audit['passed']
assert inputs['source_manifest_sha256']==hashlib.sha256((v/'source-manifest.json').read_bytes()).hexdigest()
assert all(x['manifest_sha256']==inputs['source_manifest_sha256'] for x in stages.values())
expected={x['name']:x['sha256'] for x in inputs['artifacts']}
assert expected=={x['name']:x['sha256'] for x in audit['binaries']}
rounds=[]
for n in (1,2):
 d=read(v/f'round-{n}.json');resources=read(v/f'after-round-{n}-resources.json')
 assert d['passed'] and len(d['listeners_closed'])==3
 assert {x['name']:x['exit_code'] for x in d['shutdown']}=={'server':0,'embedded':0}
 assert all(x['alive_before_signal'] and x['signal']=='SIGTERM' and x['elapsed_seconds']<10 for x in d['shutdown'])
 assert all(not x['needed'] and not x['forced_kill'] and x['exit_code']==0 for x in d['cleanup'])
 assert {f"alia-{x['name']}":x['binary_sha256'] for x in d['processes']}==expected
 assert resources['schemas']==resources['other_clients']==0
 assert d['health']['status']=='ok' and all(x['status']=='healthy' for x in d['health']['modules'].values())
 assert d['companion_policy']['policy']['mode']=='shadow'
 assert d['settings_file']['mode']=='0o600' and d['settings_file']['value']==0.25
 rounds.append({'round':n,'passed':d['passed'],'processes':d['processes'],'shutdown':d['shutdown'],'cleanup':d['cleanup'],'account_id':d['account_id'],'device_id':d['binding']['device_id'],'companion_policy':d['companion_policy'],'context_policy_etag':d['context_policy_etag'],'settings_file':d['settings_file'],'files_after_shutdown':d['files_after_shutdown'],'sqlite_integrity':d['sqlite']['integrity'],'listeners_closed':d['listeners_closed'],'resources':resources})
for k in ['account_id','device_id','companion_policy','context_policy_etag','settings_file','files_after_shutdown']:
 assert rounds[0][k]==rounds[1][k]
assert read(v/'native-inputs-after.json')['errors']==[]
assert read(e/'cleanup.json')['complete'] and read(v/'cleanup.json')['complete']
report={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'source_manifest_sha256':inputs['source_manifest_sha256'],'source_files':inputs['source_files'],'changed_inputs':inputs['changed_inputs_from_v1'],'mac_targeted':{'red':'1 passed / 1 failed / 0 ignored / 384 filtered','green':'2 passed / 0 failed / 0 ignored / 384 filtered','red_elapsed':red['elapsed_seconds'],'green_elapsed':green['elapsed_seconds']},'linux_workspace':{'totals':counts['totals'],'running_targets':counts['running_targets'],'doctest_targets':counts['doctest_targets'],'elapsed_seconds':stages['workspace-default']['elapsed_seconds']},'stage_elapsed_seconds':{k:x['elapsed_seconds'] for k,x in stages.items()},'artifacts':inputs['artifacts'],'rounds':rounds,'original_failure':{'failure':old['failure'],'shutdown':old['shutdown'],'cleanup':old['cleanup']},'resource_cleanup':{'v1':True,'v2':True},'counting_evidence':'Original Cargo log and invalid initial count preserved; one exact split heading reconstructed with line mapping in derived view; confirmed count refers to that view. No test rerun.', 'scope':'Debian 12 Linux ARM64 release, all noop, two normal SIGTERM lifecycle rounds with same V2 state; no crash/power-loss/hardware/model/physical feedback claims; original V01 root remains open'}
(r/'validation-summary.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({k:report[k] for k in ['source_files','linux_workspace','stage_elapsed_seconds','resource_cleanup']}))
