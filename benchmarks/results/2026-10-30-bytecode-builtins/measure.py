from pathlib import Path
import subprocess,json,time,random,statistics,shutil,os
b=Path(__file__).resolve().parent;root=Path(os.environ.get('TIS_SOURCE_ROOT','/Users/jaheba/p/tis'));m=Path('/Users/jaheba/p/tis');py='/opt/homebrew/Cellar/python@3.15/3.15.0/Frameworks/Python.framework/Versions/3.15/bin/python3.15';pypy='/opt/homebrew/Cellar/pypy3.10/7.3.16/bin/pypy3';rng=random.Random(20261030)
cmds={f'{v}-{t}':[str(b/'binaries'/(v+'-bench')),'sieve','--tier',t,'--warmups','1'] for v in ['before','after'] for t in ['bytecode','native']}
cmds.update({k:[exe,str(root/'tools/bench_suite.py'),'--python-worker','sieve','--warmups','1'] for k,exe in [('cpython',py),('pypy',pypy)]})
w={};r={'commands':cmds,'warmups':1,'samples':3,'ready':{},'responses':{},'samples_ns':{},'boundary':'retained VM function call; definitions/setup excluded; fresh sieve allocation, marking, scanning and output capture included'}
try:
 for k,c in cmds.items():
  p=subprocess.Popen(c,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True);w[k]=p;line=p.stdout.readline();assert line,(k,p.stderr.read());r['ready'][k]=json.loads(line);r['samples_ns'][k]=[];r['responses'][k]=[]
 for i in range(3):
  order=list(w);rng.shuffle(order)
  for k in order:
   p=w[k];p.stdin.write('run\n');p.stdin.flush();d=json.loads(p.stdout.readline());assert d['validated'];r['samples_ns'][k].append(d['run_ns']);r['responses'][k].append(d)
finally:
 for k,p in w.items():
  if p.poll() is None:p.stdin.write('quit\n');p.stdin.flush()
  p.wait();assert p.returncode==0,(k,p.stderr.read())
r['median_ms']={k:statistics.median(v)/1e6 for k,v in r['samples_ns'].items()};(b/'warmed.json').write_text(json.dumps(r,indent=2)+'\n');print('warmed',r['median_ms'],flush=True)
fc=m/'benchmarks/results/2026-10-30-flying-circus-before-after/workloads';old=m/'benchmarks/results/overnight-json-bisect-csv/workloads'
sources={'sieve':root/'tests/fixtures/sieve.py','nbody':root/'tests/fixtures/game_nbody.py','barnes-hut':root/'tests/fixtures/barnes_hut.py','ticket-search':root/'tests/fixtures/ticket_search.py','bisect-search':old/'bisect_search.py','bisect-insort':old/'bisect_insort.py'}
for name in ['perf_unpack_sequence','perf_json_loads','perf_json_dumps','text_json_parse_small','text_json_transform_small','text_json_lines_small','text_csv_aggregate_small','text_csv_cleanup_small']:sources[name]=fc/(name+'.py')
(b/'workloads').mkdir(exist_ok=True)
for n in ['before-run','after-run','monty']:subprocess.run([str(b/'binaries'/n),'--version'],capture_output=True,check=True)
oneshot={}
for name,src in sources.items():
 target=b/'workloads'/src.name;shutil.copy2(src,target);expected=subprocess.check_output([py,str(target)],text=True);commands={}
 for v in ['before','after']:
  for t in ['bytecode','native']:
   c=[str(b/'binaries'/(v+'-run')),'--tier',t]
   if name=='barnes-hut' and t=='native':c+=['--readonly-regions','--mutation-regions']
   commands[v+'-'+t]=c+[str(target)]
 commands.update({'cpython':[py,str(target)],'pypy':[pypy,str(target)],'monty':[str(b/'binaries/monty'),str(target)]})
 if name=='sieve':
  for v in ['before','after']:commands[v+'-stack']=[str(b/'binaries'/(v+'-run')),'--bytecode','stack',str(target)]
 d={'commands':commands,'expected_stdout':expected,'samples':{},'boundary':'fresh process; parsing/compilation/execution/output/exit included; no workload warmup; executable/file caches primed; Monty CLI diagnostic stderr included'}
 for i in range(3):
  order=list(commands);rng.shuffle(order)
  for k in order:
   start=time.perf_counter_ns()
   try:
    p=subprocess.run(commands[k],capture_output=True,text=True,timeout=60);ns=time.perf_counter_ns()-start
    sample={'ns':ns,'returncode':p.returncode,'stdout':p.stdout,'stderr':p.stderr,'status':'pass' if p.returncode==0 and p.stdout==expected else 'failed'}
   except subprocess.TimeoutExpired as e:sample={'ns':time.perf_counter_ns()-start,'status':'timeout'}
   d['samples'].setdefault(k,[]).append(sample)
 d['median_ms']={k:statistics.median([x['ns'] for x in a])/1e6 for k,a in d['samples'].items() if all(x['status']=='pass' for x in a)}
 d['failures']={k:[x for x in a if x['status']!='pass'] for k,a in d['samples'].items() if any(x['status']!='pass' for x in a)}
 oneshot[name]=d;(b/'oneshot.json').write_text(json.dumps(oneshot,indent=2)+'\n');print(name,d['median_ms'],'failed',list(d['failures']),flush=True)
assert not any(any(k.startswith('after-') for k in d['failures']) for d in oneshot.values()),'candidate failures remain visible in oneshot.json'
