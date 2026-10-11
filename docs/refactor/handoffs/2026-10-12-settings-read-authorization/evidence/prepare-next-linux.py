from pathlib import Path
import json,sys
r=Path(__file__).resolve().parent
version=sys.argv[1]
assert version.isdigit()
new=f'linux-arm64-v{version}'
dest=r/new/'evidence';dest.mkdir(parents=True,exist_ok=False)
for name in ['run-stage.py','prepare-source.py']:
 text=(r/'linux-arm64-v1/evidence'/name).read_text().replace('linux-20261012-v1',f'linux-20261012-v{version}')
 (dest/name).write_text(text)
command=(r/'linux-v1-command.json').read_text().replace('linux-arm64-v1/source',f'{new}/source').replace('linux-arm64-v1/evidence',f'{new}/evidence').replace('linux-arm64-v1/runtime',f'{new}/runtime').replace('linux-20261012-v1',f'linux-20261012-v{version}')
(r/f'linux-v{version}-command.json').write_text(command)
p=(r/'prepare-linux-v1.py').read_text().replace("l=r/'linux-arm64-v1'",f"l=r/'{new}'").replace('source-v1-manifest.json',f'source-v{version}-manifest.json').replace('linux-v1-command.json',f'linux-v{version}-command.json')
(r/f'prepare-linux-v{version}.py').write_text(p)
print(new)
