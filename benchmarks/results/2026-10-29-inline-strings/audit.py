import json,subprocess,hashlib
from pathlib import Path
root=Path('/Users/jaheba/p/tis')
suite=Path('/Users/jaheba/p/flying-circus/src/flying_circus/workloads')
manifest=json.loads((suite/'manifest.json').read_text())
names=json.loads((root/'benchmarks/results/2026-10-29-flying-circus-quick/supported.json').read_text())
binary=Path('/private/tmp/tis-leaf-isolated-target/release/tis')
rows=[]
for tier,extra in [('bytecode',[]),('native',['--readonly-regions','--mutation-regions'])]:
 for name in names:
  spec=manifest[name];source=suite/spec['file']
  result=subprocess.run([str(binary),'--tier',tier,*extra,str(source)],capture_output=True,text=True,timeout=30)
  row={'name':name,'tier':tier,'ok':result.returncode==0 and result.stdout==spec['stdout'] and not result.stderr,'stderr':result.stderr,'source_sha256':hashlib.sha256(source.read_bytes()).hexdigest()}
  rows.append(row)
  if not row['ok']:print(name,tier,'FAIL',result.stdout,result.stderr,flush=True)
Path('/private/tmp/tis-inline-compatibility.json').write_text(json.dumps(rows,indent=2)+'\n')
print('Passed',sum(r['ok'] for r in rows),'/',len(rows),flush=True)
assert all(r['ok'] for r in rows)
