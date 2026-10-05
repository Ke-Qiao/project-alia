#!/usr/bin/env python3
"""Exercise existing release binaries with isolated no-device configuration."""
from pathlib import Path
import datetime
import hashlib
import json
import os
import re
import secrets
import signal
import socket
import subprocess
import time
import urllib.error
import urllib.request

BASE = Path('/runtime/release-smoke-v3')
EVIDENCE = Path('/evidence')
if BASE.exists() or (EVIDENCE / 'release-smoke-summary.json').exists():
    raise SystemExit('Smoke evidence is immutable; use a fresh runtime directory.')
BASE.mkdir(mode=0o700)
ports = []
while len(ports) < 3:
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
        if port not in ports:
            ports.append(port)
http_port, server_grpc_port, device_port = ports
database_url = os.environ['ALIA_TEST_DATABASE_URL'] + '?options=-csearch_path%3Dlinux_release_smoke_v3%2Cpublic'
server_config = f'''[server]
id = "isolated-linux-review-server"
host = "127.0.0.1"
port = {http_port}
grpc_port = {server_grpc_port}
discovery_enabled = false
log_level = "error"
shutdown_grace_ms = 3000
drain_timeout_ms = 100
[database]
url = "{database_url}"
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
endpoint = "http://127.0.0.1:{device_port}"
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
[policies.memory]
enabled = false
max_retrieval_results = 10
similarity_threshold = 0.3
'''
embedded_config = f'''[device_runtime]
listen_addr = "127.0.0.1:{device_port}"
state_path = "embedded-state.sqlite3"
plugins_dir = "plugins"
shutdown_grace_ms = 3000
grpc_drain_timeout_ms = 100
[modules]
wakeword = "noop"
vad = "noop"
direct_response = "noop"
audio_capture = "noop"
audio_playback = "noop"
face_tracker = "noop"
discovery = "noop"
[runtime]
provisioned = true
[identity]
device_id = "isolated-linux-review-device"
device_name = "Isolated software fixture"
keypair_path = "embedded.key"
[discovery]
mdns_enabled = false
ble_provision_enabled = false
[motion]
enabled = false
hardware_enabled = false
[vision]
enabled = false
[hmmd_mmwave]
enabled = false
'''
for name, content in [('server.toml', server_config), ('embedded.toml', embedded_config)]:
    path = BASE / name
    path.write_text(content)
    path.chmod(0o600)
result = {'started_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
          'scope': 'Linux container release binaries; no acoustic model, device or binding mutation',
          'configuration_sha256': {name: hashlib.sha256((BASE / name).read_bytes()).hexdigest()
                                   for name in ['server.toml', 'embedded.toml']},
          'rounds': []}
processes = []
handles = []

def request(path, body=None, token=None):
    headers = {'Content-Type': 'application/json'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    req = urllib.request.Request(f'http://127.0.0.1:{http_port}' + path,
                                 data=json.dumps(body).encode() if body is not None else None,
                                 headers=headers)
    with urllib.request.urlopen(req, timeout=7) as reply:
        assert reply.status == 200
        return json.load(reply)

def wait_ready(check, proc, deadline=30):
    until = time.monotonic() + deadline
    while time.monotonic() < until:
        if proc.poll() is not None:
            raise RuntimeError('release process exited before readiness')
        try:
            return check()
        except (OSError, urllib.error.URLError, subprocess.SubprocessError):
            time.sleep(0.1)
    raise TimeoutError('release process did not become ready within 30 seconds')

def tcp_ready(port):
    with socket.create_connection(('127.0.0.1', port), timeout=1):
        return True

def start(name, round_number):
    env = os.environ.copy()
    env['RUST_LOG'] = 'error'
    env.pop('DATABASE_URL', None)
    env['ALIA_CONFIG_PATH' if name == 'server' else 'ALIA_EMBEDDED_CONFIG_PATH'] = str(BASE / (name + '.toml'))
    log = (BASE / f'{name}-{round_number}.log').open('w')
    handles.append(log)
    proc = subprocess.Popen([f'/cache/target/release/alia-{name}'], cwd=BASE, env=env,
                            stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
    processes.append(proc)
    return proc

def grpc(path, round_number, port=None):
    header = BASE / f'grpc-{round_number}-{path.rsplit("/", 1)[1]}.headers'
    cmd = ['curl', '--silent', '--show-error', '--http2-prior-knowledge', '--max-time', '5',
           '--header', 'Content-Type: application/grpc', '--header', 'TE: trailers',
           '--dump-header', str(header), '--data-binary', '@-', f'http://127.0.0.1:{port or device_port}' + path]
    response = subprocess.run(cmd, input=b'\0\0\0\0\0', capture_output=True, check=True)
    statuses = re.findall(r'^grpc-status: (\d+)\s*$', header.read_text(), flags=re.MULTILINE)
    assert len(statuses) == 1, 'one terminal gRPC status required'
    return int(statuses[0]), response.stdout

def stop(proc):
    started = time.monotonic()
    proc.send_signal(signal.SIGTERM)
    code = proc.wait(timeout=10)
    assert code == 0, 'normal signal shutdown must finish successfully'
    return {'exit_code': code, 'signal': 'SIGTERM', 'elapsed_seconds': round(time.monotonic() - started, 4)}

password = secrets.token_urlsafe(20)
original_account = None
original_policy = None
original_keys = None
try:
    for number in [1, 2]:
        device = start('embedded', number)
        wait_ready(lambda: tcp_ready(device_port), device)
        server = start('server', number)
        health = wait_ready(lambda: request('/health'), server)
        auth_body = {'email': 'isolated-linux-review@example.test', 'password': password}
        if number == 1:
            auth_body['name'] = 'Isolated Linux review'
        auth = request('/api/v1/auth/' + ('register' if number == 1 else 'login'), auth_body)
        assert auth['user']['role'] == 'admin'
        token = auth['access_token']
        if original_account is None:
            original_account = auth['user']['id']
        assert auth['user']['id'] == original_account
        policy = request('/api/v1/companion/policy', token=token)
        assert policy['policy']['mode'] == 'shadow'
        if original_policy is None:
            original_policy = policy
        assert policy == original_policy
        status = request('/api/v1/device/status?channel_id=isolated-no-device', token=token)
        assert status['online'] is True and status['device_id'] == 'isolated-linux-review-device'
        assert all(status[key] is None for key in ['cpu_percent', 'memory_percent', 'temperature_c'])
        code, binding = grpc('/alia.binding.DeviceBinding/GetBindingStatus', number)
        assert code == 0 and len(binding) >= 5 and binding[0] == 0
        assert len(binding) - 5 == int.from_bytes(binding[1:5], 'big')
        decoded = subprocess.run(['protoc', '--decode=alia.binding.BindingStatus',
                                  '--proto_path=/workspace/crates/proto/proto',
                                  '--proto_path=/usr/include',
                                  '/workspace/crates/proto/proto/binding.proto'],
                                 input=binding[5:], capture_output=True, check=True).stdout.decode()
        assert 'is_bound: true' not in decoded and 'bound_server_id:' not in decoded
        assert 'device_id: "isolated-linux-review-device"' in decoded
        auth_code, _ = grpc('/alia.playback.v3.Playback/ObserveControl', number)
        assert auth_code == 16, 'anonymous control observation must be unauthenticated'
        removed_rpc_codes = {}
        for path, port in [
            ('/alia.audio.AudioStream/StopPlayback', device_port),
            ('/alia.audio.AudioStream/PlayDirectResponse', device_port),
            ('/alia.expression.EmbeddedExpression/GetCapabilities', device_port),
            ('/alia.audio.ServerAudioIngress/InterruptActiveTurn', server_grpc_port),
        ]:
            removed_rpc_codes[path], _ = grpc(path, number, port)
            assert removed_rpc_codes[path] == 12, 'removed legacy RPC must be unimplemented'
        keys = [(BASE / name).read_bytes() for name in ['server.key', 'embedded.key']]
        if original_keys is None:
            original_keys = keys
        assert keys == original_keys
        assert (BASE / 'embedded-state.sqlite3').is_file()
        server_shutdown = stop(server)
        embedded_shutdown = stop(device)
        for port in ports:
            try:
                tcp_ready(port)
            except OSError:
                pass
            else:
                raise AssertionError('listener remains reachable after shutdown')
        result['rounds'].append({'round': number, 'health_http_status': 200,
                                'health_overall': health['status'],
                                'module_health': {key: value['status'] for key, value in health['modules'].items()},
                                'persisted_account_login': number == 2, 'same_private_keys': True,
                                'companion_mode': 'shadow', 'unchanged_policy_revision': True,
                                'device_status_online': True, 'unknown_telemetry_null': True,
                                'public_binding_status_grpc': code, 'device_unbound': True,
                                'anonymous_observe_control_grpc': auth_code,
                                'removed_rpc_grpc_codes': removed_rpc_codes,
                                'server_shutdown': server_shutdown, 'embedded_shutdown': embedded_shutdown,
                                'all_three_listeners_closed': True})
    result['exit_code'] = 0
except Exception as error:
    result['exit_code'] = 1
    result['failure_class'] = type(error).__name__
    raise
finally:
    forced_kills = 0
    for proc in processes:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                forced_kills += 1
                proc.kill()
                proc.wait()
    for handle in handles:
        handle.close()
    result['finished_at'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    result['remaining_smoke_processes'] = sum(proc.poll() is None for proc in processes)
    result['cleanup_forced_kills'] = forced_kills
    result['process_exit_codes'] = [proc.returncode for proc in processes]
    (EVIDENCE / 'release-smoke-summary.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps({'exit_code': result['exit_code'], 'completed_rounds': len(result['rounds'])}))
