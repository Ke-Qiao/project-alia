from pathlib import Path
import json,re
r=Path(__file__).resolve().parent
root=r.parents[1]
modules=['settings_read_authorization','settings_authorization']
tests={module:re.findall(r'#\[tokio::test\]\s+async fn (\w+)',(root/f'packages/alia-server/tests/server_runtime/{module}.rs').read_text()) for module in modules}
assert len(tests[modules[0]])==6 and len(tests[modules[1]])==8
results={}
for platform,path in [('macos','workspace-v3.log'),('linux','linux-arm64-v3/evidence/server-tests.log')]:
 text=(r/path).read_text()
 for module,names in tests.items():
  for name in names:
   assert f'test {module}::{name} ... ok' in text,(platform,module,name)
 results[platform]={'get_passed':len(tests[modules[0]]),'patch_passed':len(tests[modules[1]])}
(r/'authorization-regressions.json').write_text(json.dumps({'tests':tests,'results':results,'note':'Subset of full counts, never added a second time'},indent=2)+'\n')
print(json.dumps(results))
