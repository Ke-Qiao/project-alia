from pathlib import Path
import datetime, hashlib, json, os, signal, subprocess, sys, time

root = Path(subprocess.check_output(['git', 'rev-parse', '--show-toplevel'], cwd=Path(__file__).resolve().parent, text=True).strip())
out = Path(__file__).resolve().parent
phase, name, *command = sys.argv[1:]
assert phase in ('red', 'green', 'final', 'final-v2')
manifest_path = out / (phase + '-source-manifest.json')
manifest = json.loads(manifest_path.read_text())
assert name.replace('-', '').isalnum() and command
assert not (out / (name + '.json')).exists()

def drift():
    return [x['path'] for x in manifest['files']
            if not (root / x['path']).is_file()
            or hashlib.sha256((root / x['path']).read_bytes()).hexdigest() != x['sha256']]

assert not drift()
env = os.environ.copy()
for key in ['ALIA_TEST_DATABASE_URL', 'DATABASE_URL', 'RUST_MIN_STACK', 'RUST_TEST_THREADS', 'DOCS_RS']:
    env.pop(key, None)
env.update(CARGO_TERM_COLOR='never', CARGO_BUILD_JOBS='2',
           CARGO_TARGET_DIR=str(root / 'target'),
           SHERPA_ONNX_ARCHIVE_DIR=str(root / 'run/worktrees/alia-audio-clock-a33ec67/target/sherpa-onnx-prebuilt'))
use_database = (out / 'database-create.json').exists()
if use_database:
    published = json.loads((out / 'database-create.json').read_text())['published']
    env['ALIA_TEST_DATABASE_URL'] = 'postgresql://alia_test:alia_test@' + published + '/alia_v01_startup_20261015'
binary = Path(command[0])
binary_before = hashlib.sha256(binary.read_bytes()).hexdigest() if binary.is_file() else None
started = datetime.datetime.now(datetime.timezone.utc).isoformat()
clock = time.monotonic()
with (out / (name + '.log')).open('wb') as log:
    process = subprocess.Popen(command, cwd=root, env=env, stdin=subprocess.DEVNULL,
                               stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    watchdog_expired = False
    try:
        process.wait(timeout=900)
    except subprocess.TimeoutExpired:
        watchdog_expired = True
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
    result = process
record = dict(phase=phase, command=command, started_at=started, elapsed_seconds=round(time.monotonic()-clock, 2),
              exit_code=result.returncode, source_manifest_sha256=hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
              source_drift=drift(), test_parallelism='default', database='isolated task PostgreSQL available; individual targets may only use local SQLite or no DB' if use_database else 'no external database; temporary SQLite fixtures only', build_jobs=2)
record['process_watchdog_seconds'] = 900
record['process_watchdog_expired'] = watchdog_expired
record['binary_sha256'] = binary_before
record['binary_unchanged'] = binary_before == hashlib.sha256(binary.read_bytes()).hexdigest() if binary_before else None
(out / (name + '.json')).write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record))
sys.exit(result.returncode or bool(record['source_drift']))
