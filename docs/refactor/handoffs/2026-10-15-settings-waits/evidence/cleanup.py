from pathlib import Path
import datetime, json, subprocess
out = Path(__file__).resolve().parent
assert json.loads((out / 'green-server.json').read_text())['exit_code'] == 0
created = json.loads((out / 'database-create.json').read_text())
container = created['container_id']
records = []
def run(command):
    result = subprocess.run(command, text=True, capture_output=True, timeout=60)
    records.append(dict(command=command, exit_code=result.returncode, stdout=result.stdout, stderr=result.stderr))
    return result
result = run(['docker', 'inspect', '--format', '{{.Id}} {{index .Config.Labels "alia.validation"}}', container])
assert result.returncode == 0 and result.stdout.strip() == container + ' settings-waits-20261015'
result = run(['docker', 'stop', '--time', '10', container])
assert result.returncode == 0
result = run(['docker', 'inspect', '--format', '{{.State.Running}}', container])
assert result.returncode == 0 and result.stdout.strip() == 'false'
result = run(['docker', 'rm', '-v', container])
assert result.returncode == 0
result = run(['docker', 'ps', '-a', '--filter', 'id=' + container, '--format', '{{.ID}}'])
assert result.returncode == 0 and not result.stdout.strip()
run(['docker', 'ps', '--format', '{{.ID}} {{.Names}} {{.Status}}'])
(out / 'cleanup.json').write_text(json.dumps(dict(at=datetime.datetime.now(datetime.timezone.utc).isoformat(), removed_task_container=container, task_absent=True, other_containers_modified=False, shared_runtime_left_running=True, records=records), indent=2) + '\n')
print('task database removed; shared containers and runtime left alone')
