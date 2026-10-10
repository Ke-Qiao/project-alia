from pathlib import Path
import subprocess,json,time,datetime
r=Path(__file__).resolve().parent
base=['docker','exec','-i','alia-skills-20261011-db','psql','-X','-U','alia_test','-d','alia_skills_20261011','-tA','-v','ON_ERROR_STOP=1']
def sql(q): return subprocess.check_output(base+['-c',q],text=True).strip()
sql('CREATE TABLE skills_lock_snapshot_probe(value integer); INSERT INTO skills_lock_snapshot_probe VALUES(0)')
writer=reader=None
try:
 writer=subprocess.Popen(base,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,bufsize=1)
 writer.stdin.write("BEGIN; UPDATE skills_lock_snapshot_probe SET value=1; SELECT 'writer_ready';\n");writer.stdin.flush()
 seen=[]
 while True:
  line=writer.stdout.readline().strip();seen.append(line)
  if line=='writer_ready':break
 reader=subprocess.Popen(base+['-c',"SET application_name='alia_skills_lock_probe_reader'; BEGIN ISOLATION LEVEL REPEATABLE READ; LOCK TABLE skills_lock_snapshot_probe IN SHARE ROW EXCLUSIVE MODE; SELECT value FROM skills_lock_snapshot_probe; COMMIT"],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
 deadline=time.monotonic()+8;waiting=False
 while time.monotonic()<deadline:
  waiting=sql("SELECT count(*) FROM pg_stat_activity WHERE application_name='alia_skills_lock_probe_reader' AND wait_event_type='Lock'")=='1'
  if waiting:break
  time.sleep(.02)
 assert waiting,'reader did not block at table lock'
 writer.stdin.write('COMMIT;\n');writer.stdin.flush();writer.stdin.close();writer.wait(timeout=5);assert writer.returncode==0
 out,err=reader.communicate(timeout=5);assert reader.returncode==0,err
 rows=out.splitlines();assert '1' in rows and '0' not in rows,rows
 record=dict(recorded_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),database='task-only alia_skills_20261011',reader_observed_wait_event_type='Lock',reader_isolation='RepeatableRead',first_transaction_statement='LOCK TABLE ... IN SHARE ROW EXCLUSIVE MODE',reader_result_after_writer_commit=1,reader_output=rows,conclusion='The RR data snapshot is acquired after waiting for this table lock; no read occurs before the lock.')
 (r/'lock-snapshot-probe.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))
finally:
 for p in [reader,writer]:
  if p and p.poll() is None:p.kill();p.wait()
 sql('DROP TABLE skills_lock_snapshot_probe')
