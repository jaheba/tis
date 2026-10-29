from pathlib import Path
import hashlib,json,os,subprocess
root=Path('/private/tmp/tis-inline-bootstrap')
output=root/'benchmarks/results/2026-10-29-inline-entry-batch'
output.mkdir(parents=True,exist_ok=True)
files=[p for folder in ['src','examples','tests','tools'] for p in (root/folder).rglob('*') if p.is_file() and p.suffix in ['.rs','.py','.out']]+[root/'Cargo.toml',root/'Cargo.lock',root/'build.rs']
(output/'source-sha256.json').write_text(json.dumps({str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in files},indent=2)+'\n')
commands=[]
for name in ['ticket-search','barnes-hut']:
 commands.append(['python3','tools/compare_nbody.py','--baseline','/private/tmp/tis-profile/leaf-fastpath-bench','--candidate','/private/tmp/tis-profile/bootstrap-bench','--benchmark',name,'--tiers','native','--fresh-processes','--warmups','2','--output',str(output/(name+'-warm'))])
commands.append(['python3','tools/bench_oneshot.py','--baseline-run','/private/tmp/tis-profile/leaf-fastpath-run','--run','/private/tmp/tis-profile/bootstrap-run','--benchmarks','barnes-hut','ticket-search','--tiers','native','--output',str(output/'cold')])
(output/'commands.json').write_text(json.dumps(commands,indent=2)+'\n')
for command in commands:
 name=command[-1]
 print('Starting',name,flush=True)
 with open(name+'.log','w') as log:
  subprocess.run(command,cwd=root,stdout=log,stderr=subprocess.STDOUT,env=dict(os.environ,PYTHONDONTWRITEBYTECODE='1'),check=True)
 print('Completed',name,flush=True)
