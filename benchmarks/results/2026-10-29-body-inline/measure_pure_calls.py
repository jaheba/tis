import subprocess,time,random,json,hashlib,platform,statistics
from pathlib import Path
fixture=Path('benchmarks/results/2026-10-29-body-inline/pure_calls.py').resolve()
binaries={'baseline':Path('/private/tmp/tis-profile/name-load-run'),'candidate':Path('/private/tmp/tis-profile/body-inline-checked-run')}
raw=[]
randomizer=random.Random(20261029)
for i in range(10):
 order=list(binaries);randomizer.shuffle(order)
 for name in order:
  start=time.perf_counter_ns()
  result=subprocess.run([str(binaries[name]),'--tier','native',str(fixture)],text=True,capture_output=True,check=True)
  ns=time.perf_counter_ns()-start
  assert result.stdout=='ok\n',(result.stdout,result.stderr)
  raw.append(dict(label=name,round=i,run_ns=ns,validated=True))
  print(i,name,ns/1e9,flush=True)
out=Path('/private/tmp/tis-profile/body-inline-checked-micro.json')
out.write_text(json.dumps(dict(protocol='one-shot including startup/parsing/JIT/execution/exit;500 caller warmups inside each process',platform=platform.platform(),fixture_sha256=hashlib.sha256(fixture.read_bytes()).hexdigest(),binaries={k:dict(path=str(p),sha256=hashlib.sha256(p.read_bytes()).hexdigest()) for k,p in binaries.items()},samples_raw=raw,summary={name:dict(median_ns=statistics.median(r['run_ns'] for r in raw if r['label']==name)) for name in binaries}),indent=2)+'\n')
