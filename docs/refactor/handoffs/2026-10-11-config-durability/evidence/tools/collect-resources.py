from pathlib import Path
import datetime, json, subprocess, re
r = Path(__file__).resolve().parent
pg = 'alia-config-durability-20261011-db'
def sql(query):
    return subprocess.check_output(['docker', 'exec', pg, 'psql', '-U', 'alia_test', '-d', 'alia_config_durability_20261011', '-tA', '-c', query], text=True).strip()
schemas = sql("SELECT nspname FROM pg_namespace WHERE nspname NOT IN ('public','information_schema') AND nspname NOT LIKE 'pg_%' ORDER BY nspname").splitlines()
connections = int(sql("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid()"))
assert connections == 0, connections
leftover_schemas = list(schemas)
for schema in leftover_schemas:
    assert re.fullmatch(r"server_runtime_[0-9a-f]{32}", schema), schema
    sql(f'DROP SCHEMA "{schema}" CASCADE')
schemas = sql("SELECT nspname FROM pg_namespace WHERE nspname NOT IN ('public','information_schema') AND nspname NOT LIKE 'pg_%' ORDER BY nspname").splitlines()
assert not schemas, schemas
containers = {}
names = []
for creation in sorted(r.glob('linux-arm64-*/evidence/container*create.json')):
    command = json.loads(creation.read_text())['command']
    name = command[command.index('--name') + 1]
    if name not in names: names.append(name)
for name in names + [pg]:
    subprocess.run(['docker', 'stop', '--time', '5', name], check=True, stdout=subprocess.DEVNULL)
    item = json.loads(subprocess.check_output(['docker', 'inspect', name], text=True))[0]
    assert not item['State']['Running']
    containers[name] = dict(id=item['Id'], image=item['Image'], status=item['State']['Status'], exit_code=item['State']['ExitCode'], finished_at=item['State']['FinishedAt'])
record = dict(recorded_at=datetime.datetime.now(datetime.timezone.utc).isoformat(), remaining_test_schemas=schemas, leftover_schemas_removed=leftover_schemas, cleanup_reason="A killed panic-fence child may leave its task-only schema; cleanup occurs only with zero other connections", other_database_connections=connections, all_task_containers_stopped=True, containers=containers, user_database_5433='not accessed', cargo_jobs='all validation stages completed', cached_artifacts='retained in ignored run directory')
(r / 'resource-closure.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record))
