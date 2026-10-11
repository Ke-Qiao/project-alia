#!/usr/bin/env python3
"""One owned Linux validation stage; immutable source and evidence per invocation."""
from pathlib import Path
import datetime
import hashlib
import json
import subprocess
import sys
import time
import threading

RUN = Path(__file__).resolve().parent.parent
EVIDENCE = RUN / 'evidence'
SOURCE = RUN / 'source'
CONTAINER = 'alia-config-durability-linux-20261011-v1'

def timestamp():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()

def verify():
    manifest = json.loads((EVIDENCE / 'source-manifest.json').read_text())
    expected = {item['path']: item['sha256'] for item in manifest['files']}
    actual = {str(path.relative_to(SOURCE)): hashlib.sha256(path.read_bytes()).hexdigest()
              for path in SOURCE.rglob('*') if path.is_file()}
    return {'file_count': len(expected), 'errors': sorted(path for path in expected.keys() | actual.keys()
             if expected.get(path) != actual.get(path))}

if len(sys.argv) < 3:
    raise SystemExit('Usage: run-stage.py NAME COMMAND [ARG...]')
name, command = sys.argv[1], sys.argv[2:]
if '/' in name or (EVIDENCE / f'{name}.result.json').exists() or (EVIDENCE / f'{name}.log').exists():
    raise SystemExit('Stage name or evidence already exists; use a new name.')
before = verify()
if before['errors']:
    raise SystemExit('Frozen source mismatch before command.')
argv = ['docker', 'exec', '-e', 'ALIA_TEST_DATABASE_URL=postgresql://alia_test:alia_test@127.0.0.1:5432/alia_config_durability_20261011',
        CONTAINER, 'env', '-u', 'RUST_MIN_STACK', '-u', 'RUST_TEST_THREADS', '-u', 'RUST_BACKTRACE', '-u', 'DOCS_RS', 'timeout', '--signal=TERM', '--kill-after=10s', '1800', *command]
record = {'started': timestamp(), 'command': command, 'container': CONTAINER,
          'environment': {'ALIA_TEST_DATABASE_URL': 'isolated task PG; credentials omitted',
                          'CARGO_HOME': '/cache/cargo', 'CARGO_TARGET_DIR': '/cache/target',
                          'CARGO_BUILD_JOBS': '2', 'SHERPA_ONNX_ARCHIVE_DIR': '/native',
                          'RUST_TEST_THREADS': 'unset', 'RUST_MIN_STACK': 'unset', 'DOCS_RS': 'unset'},
          'manifest_sha256': hashlib.sha256((EVIDENCE / 'source-manifest.json').read_bytes()).hexdigest(),
          'source_before': before}
(EVIDENCE / f'{name}.command.json').write_text(json.dumps(record, indent=2) + '\n')
started = time.monotonic()
stop_sampling = threading.Event()
pg_samples = []
def sample_pg():
    query = "SELECT coalesce(json_agg(s),'[]'::json) FROM (SELECT pid,state,wait_event_type,wait_event,extract(epoch FROM clock_timestamp()-query_start) AS query_age_seconds,pg_blocking_pids(pid) AS blockers,left(query,160) AS query FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND state<>'idle') s"
    while not stop_sampling.is_set() and len(pg_samples) < 1800:
        sampled = {'unix_ms': int(time.time()*1000)}
        try:
            proc = subprocess.run(['docker','exec','alia-config-durability-20261011-db','psql','-U','alia_test','-d','alia_config_durability_20261011','-tA','-c',query], capture_output=True, text=True, timeout=4)
            sampled.update(exit_code=proc.returncode, activity=json.loads(proc.stdout) if proc.returncode == 0 else [], error=proc.stderr if proc.returncode else '')
        except Exception as error:
            sampled['error'] = str(error)
        pg_samples.append(sampled)
        stop_sampling.wait(1)
sampler = None
if 'tests' in name or 'panic' in name:
    sampler = threading.Thread(target=sample_pg, daemon=True)
    sampler.start()
with (EVIDENCE / f'{name}.log').open('w') as log:
    result = subprocess.run(argv, stdout=log, stderr=subprocess.STDOUT)
stop_sampling.set()
if sampler:
    sampler.join(timeout=5)
    (EVIDENCE / f'{name}.pg-waits.json').write_text(json.dumps(pg_samples, indent=2) + '\n')
record.update(exit_code=result.returncode, elapsed_seconds=round(time.monotonic() - started, 3),
              finished=timestamp(), source_after=verify())
record['finished_at'] = record['finished']
record['source_drift'] = record['source_after']['errors']
(EVIDENCE / f'{name}.result.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps({'stage': name, 'exit_code': result.returncode, 'elapsed_seconds': record['elapsed_seconds'],
                  'source_errors': record['source_after']['errors']}), flush=True)
raise SystemExit(result.returncode or int(bool(record['source_after']['errors'])))
