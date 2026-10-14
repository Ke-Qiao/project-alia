from pathlib import Path
import copy, datetime, hashlib, json, subprocess
root=Path(__file__).resolve().parents[2]
out=Path(__file__).resolve().parent
historical=root/'docs/refactor/handoffs/2026-10-14-r02-mac/evidence/synthetic-evidence.json'
original=historical.read_bytes()
fixture=json.loads(original)
first=next(copy.deepcopy(r) for r in fixture['records'] if r['timing']['frames']>0 and r['timing']['stream']['direction']=='Capture')
records=[]
for i,(quality,callback,sample) in enumerate([('BackendEstimate',1000,-1000),('Unknown',None,None),('BackendEstimate',2000,1000)]):
 r=copy.deepcopy(first)
 r['sequence']=i+1
 r['timing']['start_frame']=i*r['timing']['frames']
 r['timing']['clock'].update(quality=quality,callback_ns=callback,sample_ns=sample)
 r['lost_blocks_before']=r['lost_frames_before']=0
 records.append(r)
fixture['records']=records
fixture['observed_latest']={'capture':None,'playback':None}
path=out/'clock-transition-synthetic.json'
with path.open('x') as f:json.dump(fixture,f,separators=(',',':'));f.write('\n')
binary=root/'target/debug/examples/audio_evidence_replay'
runs=[]
for name,input_path in [('clock-transition',path),('historical-replay',historical)]:
 before=input_path.read_bytes()
 result=subprocess.run([str(binary),'replay',str(input_path)],cwd=root,capture_output=True)
 (out/(name+'.stdout.log')).write_bytes(result.stdout)
 (out/(name+'.stderr.log')).write_bytes(result.stderr)
 assert result.returncode==0,(name,result.stderr.decode())
 assert input_path.read_bytes()==before,name
 report=json.loads(result.stdout)
 assert report['source']=='SyntheticFixture' and not report['source_authenticated'] and not report['physical_stream_complete']
 if name=='clock-transition':
  assert report['records']==3 and report['observed_samples']==12
  assert report['streams'][0]['reset_counts']=={'FirstObservation':1,'ClockDiscontinuity':2}
  assert report['streams'][0]['latest_snapshot'] is None
 else:
  assert report['records']==7 and report['observed_samples']==16 and report['reset_signals']==9
 runs.append({'name':name,'exit_code':result.returncode,'input_sha256':hashlib.sha256(before).hexdigest(),'input_unchanged':True,'records':report['records'],'reset_signals':report['reset_signals']})
assert historical.read_bytes()==original
record={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'runs':runs,'all_assertions_passed':True,'scope':'synthetic evidence replay only; no native audio, algorithms, services or hardware; input fixture retained for reproducibility'}
(out/'replay-smoke.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record))
