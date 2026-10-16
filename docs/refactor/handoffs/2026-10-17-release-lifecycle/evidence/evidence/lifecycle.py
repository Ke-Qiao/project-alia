"""Bounded release process validation; private configuration is not archived."""
from pathlib import Path
import datetime
import hashlib
import json
import os
import re
import secrets
import signal
import socket
import sqlite3
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.request

BASE = Path('/runtime/staging')
EVIDENCE = Path('/evidence')
SETTING = 'core.profiles.llm.major.temperature'

def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def write_private(path, content):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as handle:
        handle.write(content)

def remaining(deadline, maximum):
    value = deadline - time.monotonic()
    if value <= 0:
        raise TimeoutError('original readiness deadline expired')
    return min(value, maximum)

def wait_ready(check, process, deadline):
    while True:
        remaining(deadline, 1)
        if process.poll() is not None:
            raise RuntimeError('release process exited before protocol readiness')
        try:
            value = check(deadline)
            remaining(deadline, 1)
            if process.poll() is not None:
                raise RuntimeError('release process exited during readiness')
            return value
        except (ConnectionError, OSError, urllib.error.URLError, subprocess.SubprocessError):
            time.sleep(remaining(deadline, 0.1))

def prepare():
    assert not BASE.exists()
    BASE.mkdir(mode=0o700)
    ports = []
    while len(ports) < 3:
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
            if port not in ports:
                ports.append(port)
    http, grpc_port, device = ports
    server = f'''[server]
id = "isolated-release-lifecycle-server"
host = "127.0.0.1"
port = {http}
grpc_port = {grpc_port}
discovery_enabled = false
log_level = "info"
shutdown_grace_ms = 3000
drain_timeout_ms = 100
[database]
url = "{os.environ['ALIA_TEST_DATABASE_URL']}"
max_connections = 4
min_connections = 0
[auth]
jwt_secret = "{secrets.token_hex(32)}"
server_keypair_path = "server.key"
jwt_expiry_hours = 1
refresh_expiry_days = 1
argon2_memory_kib = 19456
argon2_iterations = 2
argon2_parallelism = 1
[[channels]]
type = "embedded"
id = "isolated-no-device"
enabled = true
endpoint = "http://127.0.0.1:{device}"
discovery_timeout_sec = 1
reconnect_interval_sec = 1
reconnect_max_backoff_sec = 1
grpc_timeout_sec = 2
[modules]
stt = "noop"
tts = "noop"
[llm.roles]
minor = "noop"
major = "noop"
default = "noop"
embedding = "noop"
[providers.easy_turn]
enabled = false
[plugins.home_assistant]
enabled = false
'''
    template = (Path('/workspace') / 'config/embedded.staging.example.toml').read_text()
    assert template.count('127.0.0.1:0') == 1
    embedded = template.replace('127.0.0.1:0', f'127.0.0.1:{device}')
    for name, value in [('server.toml', server), ('embedded.toml', embedded)]:
        write_private(BASE / name, value)
    config = tomllib.loads(embedded)
    assert set(config['modules'].values()) == {'noop'}
    assert not config['runtime']['provisioned'] and not config['identity']['device_id']
    assert not config['dev']['skip_binding'] and not config['motion']['enabled']
    assert not config['motion']['hardware_enabled'] and not config['motion']['calibration_confirmed']
    assert not config['motion']['approved_animations']
    assert not any(config[x]['enabled'] for x in ['vision', 'vision_debug', 'hmmd_mmwave'])
    assert not config['discovery']['mdns_enabled'] and not config['discovery']['ble_provision_enabled']
    private = {'ports': ports, 'password': secrets.token_urlsafe(20)}
    write_private(BASE / 'private.json', json.dumps(private))
    report = {'at': now(), 'ports': ports, 'template_sha256': digest(Path('/workspace/config/embedded.staging.example.toml')),
              'configuration_sha256': {name: digest(BASE / name) for name in ['server.toml', 'embedded.toml']},
              'embedded': config, 'server_config_redacted': server.replace(tomllib.loads(server)['auth']['jwt_secret'], '<TASK_SECRET>'),
              'scope': 'all noop providers/devices; Server channel mDNS browse stays inside internal network; no binding or policy mutation'}
    (EVIDENCE / 'configuration-prepared.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'prepared': True, 'ports': ports}))

def run_round(number):
    path = EVIDENCE / f'round-{number}.json'
    assert number in [1, 2] and not path.exists()
    private = json.loads((BASE / 'private.json').read_text())
    http, server_grpc, device_port = private['ports']
    baseline = json.loads((EVIDENCE / 'round-1.json').read_text()) if number == 2 else None
    if baseline:
        assert baseline['passed'] and json.loads((EVIDENCE / 'after-round-1-resources.json').read_text())['other_clients'] == 0
    result = {'started_at': now(), 'round': number, 'passed': False, 'processes': [], 'cleanup': [], 'grpc': []}
    processes, handles = [], []

    def request(route, body=None, token=None, method=None, deadline=None, etag=None):
        headers = {'Content-Type': 'application/json'}
        if token:
            headers['Authorization'] = 'Bearer ' + token
        if etag:
            headers['If-Match'] = etag
        req = urllib.request.Request(f'http://127.0.0.1:{http}' + route,
                                     data=json.dumps(body).encode() if body is not None else None,
                                     headers=headers, method=method)
        with urllib.request.urlopen(req, timeout=remaining(deadline, 7) if deadline else 7) as reply:
            assert reply.status == 200
            output = json.load(reply), reply.headers.get('ETag')
        if deadline:
            remaining(deadline, 1)
        return output

    def grpc(route, port, deadline=None):
        header = EVIDENCE / f'grpc-{number}-{len(result["grpc"])}.headers'
        timeout = remaining(deadline, 5) if deadline else 5
        cmd = ['curl', '--silent', '--show-error', '--http2-prior-knowledge', '--max-time', str(timeout),
               '--header', 'Content-Type: application/grpc', '--header', 'TE: trailers',
               '--dump-header', str(header), '--data-binary', '@-', f'http://127.0.0.1:{port}' + route]
        response = subprocess.run(cmd, input=b'\0\0\0\0\0', capture_output=True, timeout=timeout, check=True)
        if deadline:
            remaining(deadline, 1)
        statuses = re.findall(r'^grpc-status: (\d+)\s*$', header.read_text(), flags=re.MULTILINE)
        assert len(statuses) == 1
        code = int(statuses[0])
        result['grpc'].append({'route': route, 'port': port, 'status': code})
        return code, response.stdout

    def binding(deadline=None):
        code, body = grpc('/alia.binding.DeviceBinding/GetBindingStatus', device_port, deadline)
        assert code == 0 and len(body) >= 5 and body[0] == 0 and len(body) - 5 == int.from_bytes(body[1:5], 'big')
        decoded = subprocess.run(['protoc', '--decode=alia.binding.BindingStatus', '--proto_path=/workspace/crates/proto/proto',
                                  '--proto_path=/usr/include', '/workspace/crates/proto/proto/binding.proto'],
                                 input=body[5:], capture_output=True, check=True,
                                 timeout=remaining(deadline, 5) if deadline else 5).stdout.decode()
        assert 'is_bound: true' not in decoded and 'bound_server_id:' not in decoded
        assert 'state:' not in decoded and 'device_public_key:' in decoded
        device_id = re.search(r'^device_id: "([^"]+)"$', decoded, flags=re.MULTILINE)
        assert device_id
        return {'device_id': device_id.group(1), 'unbound': True, 'decoded': decoded}

    def start(name):
        env = {k: v for k, v in os.environ.items() if not k.startswith('ALIA_') and k not in
               ['DATABASE_URL', 'OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'GEMINI_API_KEY', 'GOOGLE_API_KEY', 'HA_TOKEN']}
        env['RUST_LOG'] = 'info'
        key = 'ALIA_CONFIG_PATH' if name == 'server' else 'ALIA_EMBEDDED_CONFIG_PATH'
        env[key] = str(BASE / (name + '.toml'))
        log = (EVIDENCE / f'{name}-{number}.log').open('x')
        handles.append(log)
        proc = subprocess.Popen([f'/artifacts/alia-{name}'], cwd=BASE, env=env, stdin=subprocess.DEVNULL,
                                stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        processes.append((name, proc))
        result['processes'].append({'name': name, 'pid': proc.pid, 'started_at': now(), 'binary_sha256': digest(Path(f'/artifacts/alia-{name}'))})
        return proc

    def stop(name, proc):
        assert proc.poll() is None, f'{name} exited before requested shutdown'
        started = time.monotonic()
        record = {'name': name, 'pid': proc.pid, 'alive_before_signal': True, 'signal': 'SIGTERM', 'at': now()}
        result.setdefault('shutdown', []).append(record)
        proc.send_signal(signal.SIGTERM)
        record['exit_code'] = proc.wait(timeout=10)
        record['elapsed_seconds'] = round(time.monotonic() - started, 6)
        assert record['exit_code'] == 0, f'{name} normal shutdown failed'

    def files():
        config = tomllib.loads((BASE / 'embedded.toml').read_text())
        identity = config['identity']
        embedded_key = (BASE / identity['keypair_path']).resolve()
        assert embedded_key.is_relative_to(BASE)
        assert not config['runtime']['provisioned'] and not config['dev']['skip_binding']
        assert set(config['modules'].values()) == {'noop'}
        return {'server_config': digest(BASE / 'server.toml'), 'embedded_config': digest(BASE / 'embedded.toml'),
                'server_key': digest(BASE / 'server.key'), 'embedded_key': digest(embedded_key),
                'device_id': identity['device_id']}

    try:
        result['config_before'] = {name: digest(BASE / name) for name in ['server.toml', 'embedded.toml']}
        if baseline:
            assert result['config_before']['embedded.toml'] == baseline['files_after_shutdown']['embedded_config']
            assert result['config_before']['server.toml'] == baseline['files_after_shutdown']['server_config']
        device = start('embedded')
        device_deadline = time.monotonic() + 30
        result['binding'] = wait_ready(binding, device, device_deadline)
        result['device_ready_at'] = now()
        server = start('server')
        server_deadline = time.monotonic() + 30

        def health(deadline):
            body, _ = request('/health', deadline=deadline)
            if body.get('status') != 'ok' or not body.get('modules') or any(v['status'] != 'healthy' for v in body['modules'].values()):
                raise ConnectionError('runtime health is not ready')
            return body

        result['health'] = wait_ready(health, server, server_deadline)
        auth_body = {'email': 'release-lifecycle@example.test', 'password': private['password']}
        if number == 1:
            auth_body['name'] = 'Isolated release lifecycle'
        auth, _ = request('/api/v1/auth/' + ('register' if number == 1 else 'login'), auth_body, deadline=server_deadline)
        assert auth['user']['role'] == 'admin'
        token = auth['access_token']
        result['account_id'] = auth['user']['id']

        def online(deadline):
            status, _ = request('/api/v1/device/status?channel_id=isolated-no-device', token=token, deadline=deadline)
            if status['online'] is not True:
                raise ConnectionError('channel not yet online')
            assert status['device_id'] == result['binding']['device_id']
            assert all(status[x] is None for x in ['cpu_percent', 'memory_percent', 'temperature_c'])
            return status

        result['device_status'] = wait_ready(online, server, server_deadline)
        result['server_ready_at'] = now()
        result['companion_policy'], _ = request('/api/v1/companion/policy', token=token)
        assert result['companion_policy']['policy']['mode'] == 'shadow'
        settings, etag = request('/api/v1/settings?prefix=' + SETTING, token=token)
        assert settings[SETTING]['schema']['type'] == 'number' and not settings[SETTING]['schema']['requires_restart']
        result['context_policy_etag'] = etag
        assert etag is not None
        if number == 1:
            assert settings[SETTING]['value'] != 0.25
            patch, _ = request('/api/v1/settings', {SETTING: 0.25}, token, 'PATCH', etag=etag)
            result['settings_patch'] = patch
            assert patch['success'] is True and patch['errors'] == []
            assert SETTING in patch['changed'] and SETTING in patch['ignored_runtime']
            assert SETTING not in patch['applied'] and SETTING not in patch['requires_restart']
            assert patch['context_policy_version'] == int(etag.strip('"'))
        current, current_etag = request('/api/v1/settings?prefix=' + SETTING, token=token)
        assert current[SETTING]['value'] == 0.25 and current_etag == etag
        result['persisted_setting'] = {SETTING: current[SETTING]['value']}
        policy_after, _ = request('/api/v1/companion/policy', token=token)
        assert policy_after == result['companion_policy']
        if baseline:
            for field in ['account_id', 'companion_policy', 'context_policy_etag', 'binding']:
                assert result[field] == baseline[field], field + ' not recovered'
        auth_code, _ = grpc('/alia.playback.v3.Playback/ObserveControl', device_port)
        assert auth_code == 16
        for route, port in [('/alia.audio.AudioStream/StopPlayback', device_port),
                            ('/alia.audio.AudioStream/PlayDirectResponse', device_port),
                            ('/alia.expression.EmbeddedExpression/GetCapabilities', device_port),
                            ('/alia.audio.ServerAudioIngress/InterruptActiveTurn', server_grpc)]:
            code, _ = grpc(route, port)
            assert code == 12
        result['files_while_running'] = files()
        assert result['files_while_running']['device_id'] == result['binding']['device_id']
        assert result['files_while_running']['server_config'] == result['config_before']['server.toml']
        assert device.poll() is None and server.poll() is None
        stop('server', server)
        stop('embedded', device)
        closed = []
        for port in private['ports']:
            with socket.socket() as sock:
                sock.settimeout(1)
                try:
                    sock.connect(('127.0.0.1', port))
                except ConnectionRefusedError:
                    closed.append(port)
                else:
                    raise AssertionError('listener did not refuse connection after shutdown')
        result['listeners_closed'] = closed
        result['files_after_shutdown'] = files()
        assert result['files_after_shutdown'] == result['files_while_running']
        if baseline:
            assert result['files_after_shutdown'] == baseline['files_after_shutdown']
        settings_path = BASE / 'settings.toml'
        saved = tomllib.loads(settings_path.read_text())
        assert saved[SETTING] == 0.25
        result['settings_file'] = {'sha256': digest(settings_path), 'bytes': settings_path.stat().st_size,
                                   'mode': oct(settings_path.stat().st_mode & 0o777), 'value': saved[SETTING]}
        assert result['settings_file']['mode'] == '0o600'
        if baseline:
            assert result['settings_file'] == baseline['settings_file']
        config = tomllib.loads((BASE / 'embedded.toml').read_text())
        state = (BASE / config['device_runtime']['state_path']).resolve()
        assert state.is_relative_to(BASE) and state.is_file()
        db = sqlite3.connect(state.as_uri() + '?mode=ro', uri=True)
        try:
            integrity = db.execute('PRAGMA integrity_check').fetchall()
            schema = db.execute("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name").fetchall()
        finally:
            db.close()
        assert integrity == [('ok',)] and schema
        assert {'embedded_state', 'direct_responses'} <= {row[1] for row in schema if row[0] == 'table'}
        result['sqlite'] = {'path': str(state), 'integrity': integrity, 'schema': schema, 'bytes': state.stat().st_size}
        if baseline:
            assert result['sqlite']['path'] == baseline['sqlite']['path']
            assert json.loads(json.dumps(result['sqlite']['schema'])) == baseline['sqlite']['schema']
        result['passed'] = True
    except BaseException as error:
        result['failure'] = {'class': type(error).__name__, 'message': str(error)}
    finally:
        for name, proc in processes:
            cleanup = {'name': name, 'pid': proc.pid, 'needed': proc.poll() is None, 'forced_kill': False}
            try:
                if proc.poll() is None:
                    proc.terminate()
                    try:
                        proc.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        cleanup['forced_kill'] = True
                        os.killpg(proc.pid, signal.SIGKILL)
                        proc.wait(timeout=5)
                cleanup['exit_code'] = proc.returncode
            except BaseException as error:
                cleanup['error'] = repr(error)
            if cleanup['needed'] or cleanup.get('error') or cleanup['exit_code'] != 0:
                result['passed'] = False
            result['cleanup'].append(cleanup)
        for handle in handles:
            handle.close()
        result['finished_at'] = now()
        path.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'round': number, 'passed': result['passed'], 'failure': result.get('failure')}))
    raise SystemExit(0 if result['passed'] else 1)

if __name__ == '__main__':
    if sys.argv[1:] == ['prepare']:
        prepare()
    else:
        run_round(int(sys.argv[1]))
