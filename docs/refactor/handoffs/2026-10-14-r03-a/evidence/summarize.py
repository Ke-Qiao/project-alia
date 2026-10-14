from pathlib import Path
import datetime, hashlib, json, re
out=Path(__file__).resolve().parent
names=['green-core-transport','green-embedded','green-server']
runs=[]
for name in names:
 record=json.loads((out/(name+'.json')).read_text())
 assert record['exit_code']==0 and record['source_drift']==[]
 targets=[]
 for block in re.split(r'^\s*Running ',(out/(name+'.log')).read_text(),flags=re.M)[1:]:
  matches=re.findall(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out',block)
  if matches:
   values=matches[-1]
   targets.append(dict(target=block.splitlines()[0],passed=int(values[0]),failed=int(values[1]),ignored=int(values[2]),filtered=int(values[4])))
 totals={key:sum(t[key] for t in targets) for key in ['passed','failed','ignored','filtered']}
 assert totals['failed']==0 and totals['filtered']==0
 counts={'targets':targets,'totals':totals,'method':'last test result per Cargo Running target; nested child summaries excluded'}
 (out/(name+'-counts.json')).write_text(json.dumps(counts,indent=2)+'\n')
 runs.append(dict(name=name,**record,totals=totals))
summary={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'source_manifest':'green-source-manifest.json','source_manifest_sha256':hashlib.sha256((out/'green-source-manifest.json').read_bytes()).hexdigest(),'inputs':len(json.loads((out/'green-source-manifest.json').read_text())['files']),'platform':'macOS26.6.2 ARM64 / Rust1.95','runs':runs,'aggregate':{key:sum(r['totals'][key] for r in runs) for key in ['passed','failed','ignored','filtered']},'not_run':['Linux application/Embedded tests','Pi/hardware/real audio','real PTY3 and model integration2 ignored tests','release builds and long-running load'], 'prior_failures':['three initial assertion RED','cue upload boundary assertion RED','candidate Server DB unavailable; owned test child terminated, original log retained'], 'environment_notes':['OrbStack startup auto-restarted three existing services; exact services restored stopped; no application requests sent','task PostgreSQL clean-stopped before first candidate Server test, initiator unknown; SELECT1 verified after restart; successful retry recorded'], 'formatting':'changed Rust passed; workspace fmt only unchanged baseline Server import ordering'}
(out/'validation-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary['aggregate']))
