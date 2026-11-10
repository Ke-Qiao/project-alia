import pathlib,json,os,socket,subprocess,time,urllib.request,signal
s=json.loads(pathlib.Path('/private/tmp/alia-infra-fixes-state.json').read_text());scratch=pathlib.Path(s['snapshot']);snap=scratch/'snapshot';logs=pathlib.Path(s['logs'])
def bindable(port):
 with socket.socket() as sock:
  try:sock.bind(('127.0.0.1',port));return True
  except OSError:return False
assert bindable(3100), 'Port 3100 is already in use; refusing to touch another process'
backend_was_free=bindable(3000)
env=os.environ.copy();env['NEXT_TELEMETRY_DISABLED']='1';env['PATH']=str(scratch/'bin')+os.pathsep+env['PATH']
with (logs/'web-dev-server.log').open('w') as f:
 p=subprocess.Popen(['pnpm','web:dev','--hostname','127.0.0.1'],cwd=snap,env=env,stdout=f,stderr=subprocess.STDOUT,start_new_session=True)
 try:
  until=time.monotonic()+45
  while time.monotonic()<until:
   assert p.poll() is None, 'Next exited before readiness'
   if 'Ready' in (logs/'web-dev-server.log').read_text():break
   time.sleep(.25)
  else:raise RuntimeError('Next did not become ready in 45 seconds')
  log=(logs/'web-dev-server.log').read_text();print(log)
  assert '127.0.0.1:3100' in log, 'Unexpected server port'
  with urllib.request.urlopen('http://127.0.0.1:3100',timeout=15) as response:
   body=response.read();assert response.status==200 and b'Alia' in body
   print('Web / returned HTTP 200 on 3100')
  if backend_was_free:
   assert bindable(3000), 'Web occupied backend port 3000'
   print('Backend HTTP port 3000 remains available while Web runs')
  else:print('Port 3000 was already occupied before this check; existing service untouched')
 finally:
  os.killpg(p.pid,signal.SIGTERM)
  try:p.wait(timeout=5)
  except subprocess.TimeoutExpired:os.killpg(p.pid,signal.SIGKILL);p.wait()
print('Stopped only the temporary Web process group')
