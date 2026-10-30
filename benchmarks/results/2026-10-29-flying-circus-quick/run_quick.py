import os,subprocess,json
from pathlib import Path
suite=Path('/Users/jaheba/p/flying-circus/src/flying_circus/workloads');manifest=json.loads((suite/'manifest.json').read_text())
names=json.load(open('/private/tmp/tis-circus-supported.json'));names=[n for n in names if manifest[n].get('suite')!='internal']
command=['/Users/jaheba/.local/share/uv/tools/flying-circus/bin/python','-m','flying_circus','--quick','-v','-x1','-r','tis-bytecode=/private/tmp/tis-profile/circus','-r','tis-native=/private/tmp/tis-profile/circus','-r','cpython=/opt/homebrew/bin/python3','-r','pypy=/opt/homebrew/bin/pypy3',
 '--runtime-arg','tis-bytecode=--tier','--runtime-arg','tis-bytecode=bytecode',
 '--runtime-arg','tis-native=--tier','--runtime-arg','tis-native=native','--runtime-arg','tis-native=--readonly-regions','--runtime-arg','tis-native=--mutation-regions',
 '--format','html','--format','markdown','--format','json','--output','/private/tmp/tis-circus-quick-final']
for name in names:command+=['--benchmark',str(suite/manifest[name]['file'])]
Path('/private/tmp/tis-circus-final-command.json').write_text(json.dumps(command,indent=2)+'\n')
env=os.environ.copy();env['PYTHONPATH']='/private/tmp/flying-circus-tis/src'
with open('/private/tmp/tis-circus-quick-final.log','w') as log:
 result=subprocess.run(command,env=env,stdout=log,stderr=subprocess.STDOUT,cwd='/private/tmp/flying-circus-tis')
print('Flying Circus exit',result.returncode,flush=True)
raise SystemExit(result.returncode)
