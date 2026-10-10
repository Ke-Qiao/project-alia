from pathlib import Path
import subprocess
r = Path(__file__).resolve().parent
code = '''from pathlib import Path
import hashlib, json, shutil
origin = Path('/cache/target/debug/deps/server_runtime-d688634c1b44968f')
dest = Path('/runtime/server_runtime-v4')
assert not dest.exists()
shutil.copy2(origin, dest)
def sha(p):
    with p.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()
original_hash, retained_hash = sha(origin), sha(dest)
assert original_hash == retained_hash
record = dict(original=str(origin), retained=str(dest), bytes=origin.stat().st_size, original_mtime_ns=origin.stat().st_mtime_ns, sha256=retained_hash, purpose='Retain V4 panic-fence failing/rechecked test executable for later diagnosis; ignored task runtime artifact, not deployed')
Path('/evidence/retained-runtime-binary.json').write_text(json.dumps(record, indent=2) + '\\n')
print(json.dumps(record))
'''
subprocess.run(['python3', str(r / 'linux-arm64-v4/evidence/run-stage.py'), 'retain-runtime-binary', 'python3', '-c', code], check=True)
