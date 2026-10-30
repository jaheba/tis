from pathlib import Path
import subprocess,json,time,random,statistics,shutil
b=Path(__file__).resolve().parent;root=Path('/private/tmp/tis-sieve-opt');py='/opt/homebrew/Cellar/python@3.15/3.15.0/Frameworks/Python.framework/Versions/3.15/bin/python3.15';pypy='/opt/homebrew/Cellar/pypy3.10/7.3.16/bin/pypy3';rng=random.Random(20261030)
commands={f'{version}-{tier}':[str(b/'binaries'/(version+'-bench')),'sieve','--tier',tier,'--warmups','1'] for version in ['before','after'] for tier in ['bytecode','native']}
commands.update({name:[exe,str(root/'tools/bench_suite.py'),'--python-worker','sieve','--warmups','1'] for name,exe in [('cpython',py),('pypy',pypy)]})
r={'commands':commands,'warmups':1,'samples':3,'boundary':'retained VM function invocation; allocation/marking/scanning/printing included; definitions/setup excluded','ready':{},'samples_ns':{},'responses':{},'stderr':{}}
workers={}
try:
 for name,cmd in commands.items():
  p=subprocess.Popen(cmd,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,bufsize=1);workers[name]=p
  line=p.stdout.readline();assert line,(name,p.stderr.read());r['ready'][name]=json.loads(line);r['samples_ns'][name]=[];r['responses'][name]=[]
 for _ in range(3):
  order=list(workers);rng.shuffle(order)
  for name in order:
   p=workers[name];p.stdin.write('run\n');p.stdin.flush();v=json.loads(p.stdout.readline());assert v['validated'],v;r['samples_ns'][name].append(v['run_ns']);r['responses'][name].append(v)
finally:
 for name,p in workers.items():
  if p.poll() is None:p.stdin.write('quit\n');p.stdin.flush()
  p.wait();r['stderr'][name]=p.stderr.read();assert p.returncode==0,(name,r['stderr'][name])
r['median_ms']={k:statistics.median(v)/1e6 for k,v in r['samples_ns'].items()};(b/'warmed.json').write_text(json.dumps(r,indent=2)+'\n');print('warmed',r['median_ms'],flush=True)
# Fresh processes, with executable verification/file caches primed by --version only.
for n in ['before-run','after-run']:subprocess.run([str(b/'binaries'/n),'--version'],check=True,capture_output=True)
sources={'sieve':root/'tests/fixtures/sieve.py','nbody':root/'tests/fixtures/game_nbody.py','barnes-hut':root/'tests/fixtures/barnes_hut.py','unpacking':Path('/Users/jaheba/p/tis/benchmarks/results/2026-10-30-flying-circus-before-after/workloads/perf_unpack_sequence.py')}
(b/'workloads').mkdir(exist_ok=True);oneshot={}
for name,src in sources.items():
 target=b/'workloads'/src.name;shutil.copy2(src,target);expected=subprocess.check_output([py,str(target)],text=True);cmds={}
 for version in ['before','after']:
  for tier in ['bytecode','native']:
   cmd=[str(b/'binaries'/(version+'-run')),'--tier',tier]
   if name=='barnes-hut' and tier=='native':cmd+=['--readonly-regions','--mutation-regions']
   cmds[version+'-'+tier]=cmd+[str(target)]
 cmds['cpython']=[py,str(target)];cmds['pypy']=[pypy,str(target)]
 data={'commands':cmds,'expected_stdout':expected,'samples':{},'boundary':'fresh process including parsing, compilation, execution, stdout delivery and exit; no workload warmup; OS caches primed'}
 for _ in range(3):
  order=list(cmds);rng.shuffle(order)
  for label in order:
   start=time.perf_counter_ns();p=subprocess.run(cmds[label],capture_output=True,text=True);ns=time.perf_counter_ns()-start
   assert p.returncode==0 and p.stdout==expected,(name,label,p.returncode,p.stdout,p.stderr)
   data['samples'].setdefault(label,[]).append(dict(ns=ns,stderr=p.stderr))
 data['median_ms']={k:statistics.median([v['ns'] for v in rows])/1e6 for k,rows in data['samples'].items()};oneshot[name]=data
 (b/'oneshot.json').write_text(json.dumps(oneshot,indent=2)+'\n');print(name,data['median_ms'],flush=True)
