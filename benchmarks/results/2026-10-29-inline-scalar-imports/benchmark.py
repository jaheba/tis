from pathlib import Path
import hashlib,json,os,subprocess
root=Path('/private/tmp/tis-fix-lir');out=root/'benchmarks/results/2026-10-29-inline-scalar-imports';out.mkdir(parents=True,exist_ok=True)
files=[p for folder in ['src','examples','tests','tools','experiments'] for p in (root/folder).rglob('*') if p.is_file() and p.suffix in ['.rs','.py','.out']]+[root/'Cargo.toml',root/'Cargo.lock',root/'build.rs']
(out/'source-sha256.json').write_text(json.dumps({str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in files},indent=2)+'\n')
commands=[]
for benchmark,experimental in [('barnes-hut',True),('barnes-hut',False),('ticket-search',True)]:
 command=['python3','tools/compare_nbody.py','--baseline','/private/tmp/tis-profile/shared-bench','--candidate','/private/tmp/tis-profile/fix-lir-bench','--benchmark',benchmark,'--tiers','native','--samples','1','--warmups','2' if benchmark=='barnes-hut' else '1']
 if experimental:command+=['--baseline-cfg-inlining','--candidate-cfg-inlining']
 command+=['--output',str(out/(benchmark+('-experimental' if experimental else '-default')))]
 commands.append(command)
(out/'commands.json').write_text(json.dumps(commands,indent=2)+'\n')
for command in commands:
 print('Starting',command[-1],flush=True)
 with open(command[-1]+'.log','w') as log:subprocess.run(command,cwd=root,stdout=log,stderr=subprocess.STDOUT,env=dict(os.environ,PYTHONDONTWRITEBYTECODE='1'),check=True)
 data=json.loads((Path(command[-1])/'comparison.json').read_text())
 assert all(s['validated'] and s['backend_compile_ns']==0 and s['deoptimizations']==0 for s in data['samples_raw'])
 assert all(s['compilation_errors_total']==0 for s in data['samples_raw'] if s['label']=='candidate/native')
 print('Completed',command[-1],flush=True)
