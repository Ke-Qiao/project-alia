from pathlib import Path
import datetime,hashlib,json,subprocess,tempfile
root=Path(__file__).resolve().parents[2];out=Path(__file__).resolve().parent
binary=root/'target/debug/examples/audio_evidence_replay'
fixture=out/'synthetic-evidence.json';results=[]
def run(name,args,cwd,code):
 r=subprocess.run([str(binary),*map(str,args)],cwd=cwd,capture_output=True,timeout=15)
 (out/(name+'.stdout.log')).write_bytes(r.stdout);(out/(name+'.stderr.log')).write_bytes(r.stderr)
 item={'name':name,'args':list(map(str,args)),'exit_code':r.returncode,'expected_exit_code':code}
 results.append(item);assert r.returncode==code,(item,r.stderr.decode())
 return r
with tempfile.TemporaryDirectory(prefix='alia-r02-cli-') as temp:
 temp=Path(temp)
 help=run('help',['--help'],temp,0);assert b'generate' in help.stdout
 run('implicit-rejected',[temp/'implicit.json'],temp,1)
 run('extra-rejected',['generate',temp/'extra.json','unexpected'],temp,1)
 run('missing-rejected',['replay',temp/'missing.json'],temp,1)
 assert not list(temp.iterdir())
 generated=run('generate',['generate',fixture],temp,0)
 before=fixture.read_bytes();before_names=sorted(p.name for p in out.iterdir())
 replayed=run('replay',['replay',fixture],temp,0)
 assert json.loads(replayed.stdout)==json.loads(generated.stdout)
 assert fixture.read_bytes()==before
 before_overwrite=sorted(p.name for p in out.iterdir())
 run('no-overwrite',['generate',fixture],temp,1)
 assert fixture.read_bytes()==before
 assert sorted(p.name for p in out.iterdir())==sorted(before_overwrite+['no-overwrite.stdout.log','no-overwrite.stderr.log'])
 old=root/'docs/refactor/handoffs/2026-10-14-field-prep/evidence/mac-synthetic-evidence.json'
 old_bytes=old.read_bytes();historical=run('historical-replay',['replay',old],temp,0)
 assert old.read_bytes()==old_bytes
 assert json.loads(historical.stdout)==json.loads(replayed.stdout)
 corrupt=temp/'corrupt.json';data=json.loads(before);data['observed_latest']['capture']['closed']=True;data['observed_latest']['capture']['running']=False
 data['observed_latest']['capture']['lost_blocks']=data['observed_latest']['capture']['attempted_blocks']
 corrupt.write_text(json.dumps(data));corrupt_before=corrupt.read_bytes()
 rejected=run('contradictory-closed-rejected',['replay',corrupt],temp,1)
 assert b'account for observed records' in rejected.stderr
 assert corrupt.read_bytes()==corrupt_before
 summary=json.loads(replayed.stdout)
 assert summary['records']==7 and summary['observed_samples']==16 and summary['reset_signals']==9
 assert summary['source_authenticated'] is False and summary['physical_stream_complete'] is False
 assert len(summary['streams'])==3
record={'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'source_manifest':'final-source-manifest.json','fixture_sha256':hashlib.sha256(before).hexdigest(),'runs':results,'all_assertions_passed':True,'temporary_directory_removed':not temp.exists(),'scope':'offline CLI only; no audio devices, models, services or execution runtime'}
(out/'cli-smoke.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record))
