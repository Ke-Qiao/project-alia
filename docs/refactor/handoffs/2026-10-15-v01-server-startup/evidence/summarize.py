from pathlib import Path
import datetime,hashlib,json,re,shutil
out=Path(__file__).resolve().parent
root=out.parent.parent
record=json.loads((out/'final-workspace.json').read_text())
assert record['exit_code']==0 and not record['source_drift'] and not record['process_watchdog_expired']
log=(out/'final-workspace.log').read_text()
headers=list(re.finditer(r'^\s*(Running |Doc-tests )([^\n]+)',log,re.M))
targets=[];ignored=[]
for i,h in enumerate(headers):
    block=log[h.end():headers[i+1].start() if i+1<len(headers) else len(log)]
    results=re.findall(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out',block)
    assert results,h.group(2)
    targets.append(dict(kind=h.group(1).strip(),target=h.group(2),**dict(zip(['passed','failed','ignored','measured','filtered'],map(int,results[-1])))))
    ignored+=re.findall(r'^test (.+?) \.\.\. ignored(?:, (.*))?$',block,re.M)
totals={k:sum(t[k] for t in targets) for k in ['passed','failed','ignored','measured','filtered']}
assert totals['failed']==totals['measured']==totals['filtered']==0
assert totals['ignored']==len(ignored)==5
assert totals['passed']==1270 and len(targets)==72
paired=[t for t in targets if t['target'].startswith('tests/server_runtime.rs ')];assert len(paired)==1 and paired[0]['passed']==85
binary_path=re.search(r'\(([^)]+)\)',paired[0]['target']).group(1)
binary=root/binary_path;retained=out/'final-server-runtime';assert not retained.exists();shutil.copy2(binary,retained)
digest=hashlib.sha256(retained.read_bytes()).hexdigest();assert digest==hashlib.sha256(binary.read_bytes()).hexdigest()
manifest=out/'final-source-manifest.json'
summary=dict(at=datetime.datetime.now(datetime.timezone.utc).isoformat(),run=record,totals=totals,targets=targets,ignored=ignored,server_runtime=paired[0],new_regressions=4,source_inputs=len(json.loads(manifest.read_text())['files']),manifest_sha256=hashlib.sha256(manifest.read_bytes()).hexdigest(),binary=dict(original_path=binary_path,retained_path=str(retained.relative_to(root)),sha256=digest,bytes=retained.stat().st_size),historical_v01_status='open',counting='Final complete run only; last summary per Cargo target; child reentry, RED and targeted GREEN are not added',limits=['No Linux/Pi/hardware/model execution','Buffered ServerStartup ready cancellation and selected partial initialization cases are covered; early connect, SQL confirmation, discovery failure, runtime teardown and exhaustive native/supervisor failure injection are not','Production supervisor running flag can remain set after panic; never count watchdog as cleanup'])
(out/'validation-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(dict(totals=totals,running_targets=sum(t['kind']=='Running' for t in targets),doc_targets=sum(t['kind']=='Doc-tests' for t in targets),server_runtime=paired[0],binary_sha256=digest)))
