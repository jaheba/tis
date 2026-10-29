from pathlib import Path
import hashlib,json,os,subprocess
root=Path('/private/tmp/tis-shared-lowering')
out=root/'benchmarks/results/2026-10-29-shared-inline-lowering'
out.mkdir(parents=True,exist_ok=True)
files=[p for folder in ['src','examples','tests','tools','experiments'] for p in (root/folder).rglob('*') if p.is_file() and p.suffix in ['.rs','.py','.out']]+[root/'Cargo.toml',root/'Cargo.lock',root/'build.rs']
(out/'source-sha256.json').write_text(json.dumps({str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in files},indent=2)+'\n')
commands=[]
for benchmark,experiment in [('barnes-hut',True),('barnes-hut',False),('ticket-search',True)]:
 command=['python3','tools/compare_nbody.py','--baseline','/private/tmp/tis-profile/cfg-bench','--candidate','/private/tmp/tis-profile/shared-bench','--benchmark',benchmark,'--tiers','native','--samples','1','--warmups','2' if benchmark=='barnes-hut' else '1']
 if experiment:command+=['--baseline-cfg-inlining','--candidate-cfg-inlining']
 command+=['--output',str(out/(benchmark+('-experimental' if experiment else '-default')))]
 commands.append(command)
commands.append(['python3','tools/bench_oneshot.py','--baseline-run','/private/tmp/tis-profile/shared-run','--run','/private/tmp/tis-profile/shared-run','--cfg-inlining','--benchmarks','barnes-hut','--tiers','native','--samples','1','--output',str(out/'cold-cfg-off-on')])
(out/'commands.json').write_text(json.dumps(commands,indent=2)+'\n')
for command in commands:
 print('Starting',command[-1],flush=True)
 with open(command[-1]+'.log','w') as log:
  subprocess.run(command,cwd=root,stdout=log,stderr=subprocess.STDOUT,env=dict(os.environ,PYTHONDONTWRITEBYTECODE='1'),check=True)
 print('Completed',command[-1],flush=True)
