import sys,os,subprocess,json
from pathlib import Path
root=Path('/Users/jaheba/p/tis/benchmarks/results/overnight-json-bisect-csv')
label=sys.argv[1];binary=sys.argv[2]
output=Path('/private/tmp')/('tis-overnight-'+label)
cmd=['/Users/jaheba/.local/share/uv/tools/flying-circus/bin/python','-m','flying_circus','--quick','-v','-x1','-r','monty='+str(root/'binaries/monty'),'-r','tis-bytecode='+binary,'-r','tis-native='+binary,'-r','cpython=/opt/homebrew/bin/python3',
'--runtime-arg','tis-bytecode=--tier','--runtime-arg','tis-bytecode=bytecode','--runtime-arg','tis-native=--tier','--runtime-arg','tis-native=native','--runtime-arg','tis-native=--readonly-regions','--runtime-arg','tis-native=--mutation-regions', '--format','html','--format','markdown','--format','json','--output',str(output)]
if label.startswith('confirm'): cmd += ['--samples', '6', '--startup-samples', '3']
if label.startswith('final'): cmd += ['--samples', '12', '--startup-samples', '3']
selected = set(sys.argv[3:])
for p in sorted((root/'workloads').glob('*.py')):
 if not selected or p.stem in selected: cmd += ['--benchmark',str(p)]
Path('/private/tmp/overnight-'+label+'-command.json').write_text(json.dumps(cmd,indent=2)+'\n')
env=os.environ.copy();env['PYTHONPATH']='/private/tmp/flying-circus-tis/src'
with open('/private/tmp/overnight-'+label+'.log','w') as f:result=subprocess.run(cmd,env=env,stdout=f,stderr=subprocess.STDOUT,cwd='/private/tmp/flying-circus-tis')
print('Flying Circus',label,'exit',result.returncode)
raise SystemExit(result.returncode)
