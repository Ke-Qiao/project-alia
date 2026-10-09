from pathlib import Path
import json,subprocess,hashlib,datetime
r=Path(__file__).resolve().parent
records=[]
for cmd in [['rustc','-Vv'],['cargo','-V'],['uname','-sm'],['sw_vers']]:
 p=subprocess.run(cmd,capture_output=True,text=True,check=True);records.append(dict(command=cmd,exit_code=p.returncode,stdout=p.stdout,stderr=p.stderr))
(r/'macos-environment.json').write_text(json.dumps(records,indent=2)+'\n')
for archive,dest,expected in [
(Path('run/worktrees/alia-audio-clock-a33ec67/target/sherpa-onnx-prebuilt/sherpa-onnx-v1.13.1-osx-arm64-static-lib.tar.bz2'),r/'macos-native-archive.json','ca7baf61f331ea6b848e3630e566647302c26960c7303932bc33125074871eeb'),
(Path('run/validation-20261008-layering/linux-arm64-v1/native/sherpa-onnx-v1.13.1-linux-aarch64-static-lib.tar.bz2'),r/'linux-arm64-v2/evidence/native-archive.json','86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749')]:
 sha=hashlib.sha256(archive.read_bytes()).hexdigest();assert sha==expected
 dest.write_text(json.dumps(dict(archive=archive.name,bytes=archive.stat().st_size,sha256=sha,verified_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),usage='Existing archive, default-feature native linking; no physical audio I/O.'),indent=2)+'\n')
