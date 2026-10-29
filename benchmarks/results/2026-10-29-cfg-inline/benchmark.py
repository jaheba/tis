from pathlib import Path
import hashlib,json,os,subprocess
root=Path('/private/tmp/tis-cfg-inline')
out=root/'benchmarks/results/2026-10-29-cfg-inline'
out.mkdir(parents=True,exist_ok=True)
files=[p for folder in ['src','examples','tests','tools','experiments'] for p in (root/folder).rglob('*') if p.is_file() and p.suffix in ['.rs','.py','.out']]+[root/'Cargo.toml',root/'Cargo.lock',root/'build.rs']
(out/'source-sha256.json').write_text(json.dumps({str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in files},indent=2)+'\n')
commands=[]
for name in ['ticket-search','barnes-hut']:
 for experiment in [False,True]:
  command=['python3','tools/compare_nbody.py','--baseline','/private/tmp/tis-profile/cfg-bench' if experiment else '/private/tmp/tis-profile/bootstrap-bench','--candidate','/private/tmp/tis-profile/cfg-bench','--benchmark',name,'--tiers','native','--fresh-processes','--warmups','2']
  if experiment:command+=['--candidate-cfg-inlining']
  command+=['--output',str(out/(name+('-experiment' if experiment else '-default')))]
  commands.append(command)
commands.append(['python3','tools/bench_oneshot.py','--baseline-run','/private/tmp/tis-profile/cfg-run','--run','/private/tmp/tis-profile/cfg-run','--cfg-inlining','--benchmarks','barnes-hut','ticket-search','--tiers','native','--output',str(out/'cold-experiment')])
(out/'commands.json').write_text(json.dumps(commands,indent=2)+'\n')
for command in commands:
 name=command[-1]
 print('Starting',name,flush=True)
 with open(name+'.log','w') as log:
  subprocess.run(command,cwd=root,stdout=log,stderr=subprocess.STDOUT,env=dict(os.environ,PYTHONDONTWRITEBYTECODE='1'),check=True)
 print('Completed',name,flush=True)
