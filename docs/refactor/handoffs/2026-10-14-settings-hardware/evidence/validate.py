from pathlib import Path
import datetime, hashlib, json, os, subprocess, sys, time

root = Path(__file__).resolve().parents[2]
out = Path(__file__).resolve().parent
manifest = json.loads((out / 'source-manifest.json').read_text())
name, *command = sys.argv[1:]
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
started = datetime.datetime.now(datetime.timezone.utc).isoformat()
clock = time.monotonic()
with (out / (name + '.log')).open('wb') as log:
    result = subprocess.run(command, cwd=root, env=env, stdin=subprocess.DEVNULL,
                            stdout=log, stderr=subprocess.STDOUT)
record = dict(command=command, started_at=started, elapsed_seconds=round(time.monotonic()-clock, 2),
              exit_code=result.returncode, source_manifest_sha256=hashlib.sha256((out/'source-manifest.json').read_bytes()).hexdigest(),
              source_drift=drift(), test_parallelism='default', database='not used', build_jobs=2)
(out / (name + '.json')).write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record))
sys.exit(result.returncode or bool(record['source_drift']))
