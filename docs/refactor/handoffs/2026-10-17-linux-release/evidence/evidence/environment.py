import hashlib,json,os,pathlib,subprocess
commands=[['uname','-a'],['rustc','-Vv'],['cargo','-V'],['protoc','--version'],['cat','/etc/os-release'],['pkg-config','--modversion','alsa','dbus-1','openssl'],['dpkg-query','-W','libasound2-dev','libdbus-1-dev','libssl-dev','libc6','libstdc++6','protobuf-compiler'],['ldd','--version']]
report={'commands':[],'environment':{k:os.environ.get(k) for k in ['ALIA_TEST_DATABASE_URL','DATABASE_URL','CARGO_HOME','CARGO_TARGET_DIR','CARGO_BUILD_JOBS','RUST_TEST_THREADS','RUST_MIN_STACK','RUST_BACKTRACE','DOCS_RS','SHERPA_ONNX_ARCHIVE_DIR','SHERPA_ONNX_LIB_DIR']},'native_system_libraries':[]}
for command in commands:
 p=subprocess.run(command,capture_output=True,text=True,timeout=30)
 report['commands'].append({'command':command,'exit_code':p.returncode,'stdout':p.stdout,'stderr':p.stderr})
 if p.returncode: raise RuntimeError(str(command)+p.stderr)
for name in ['libasound.so.2','libdbus-1.so.3','libssl.so.3','libcrypto.so.3','libc.so.6','libstdc++.so.6','libgcc_s.so.1','libm.so.6','ld-linux-aarch64.so.1']:
 path=pathlib.Path('/lib/aarch64-linux-gnu')/name
 report['native_system_libraries'].append({'name':name,'path':str(path.resolve()),'sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'bytes':path.stat().st_size})
assert report['environment']['DATABASE_URL'] is None
assert all(report['environment'][x] is None for x in ['RUST_TEST_THREADS','RUST_MIN_STACK','DOCS_RS','SHERPA_ONNX_LIB_DIR'])
assert report['environment']['ALIA_TEST_DATABASE_URL']=='postgresql://alia_test:alia_test@127.0.0.1:5432/alia_linux_release_20261017'
print(json.dumps(report,indent=2))
