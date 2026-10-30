from pathlib import Path
import subprocess,os,json,time,random,statistics,tempfile,shutil,re
b=Path(__file__).resolve().parent
py='/opt/homebrew/Cellar/python@3.15/3.15.0/Frameworks/Python.framework/Versions/3.15/bin/python3.15'
pypy='/opt/homebrew/Cellar/pypy3.10/7.3.16/bin/pypy3'
rng=random.Random(20261031)
def sample(command,expected):
 with tempfile.TemporaryFile() as stdout,tempfile.TemporaryFile() as stderr:
  start=time.perf_counter_ns();p=subprocess.Popen(command,stdout=stdout,stderr=stderr);_,status,usage=os.wait4(p.pid,0);elapsed=time.perf_counter_ns()-start;p.returncode=os.waitstatus_to_exitcode(status)
  stdout.seek(0);output=stdout.read().decode();stderr.seek(0);err=stderr.read().decode()
 assert p.returncode==0 and output==expected,(command,p.returncode,output,err)
 return {'elapsed_ns':elapsed,'rss_bytes':usage.ru_maxrss,'user_s':usage.ru_utime,'system_s':usage.ru_stime,'stdout':output,'stderr':err,'stats':{k:int(v) for k,v in re.findall(r'([a-z_]+): (\d+)',err)}}
def measure(commands,expected,samples=3):
 d={'commands':commands,'expected_stdout':expected,'samples':{k:[] for k in commands}}
 for _ in range(samples):
  order=list(commands);rng.shuffle(order)
  for key in order:d['samples'][key].append(sample(commands[key],expected))
 d['median_ms']={k:statistics.median(x['elapsed_ns'] for x in values)/1e6 for k,values in d['samples'].items()}
 d['median_rss_mib']={k:statistics.median(x['rss_bytes'] for x in values)/2**20 for k,values in d['samples'].items()}
 print(d['median_ms'],flush=True);return d
for variant in ['before','after']:subprocess.run([str(b/'binaries'/(variant+'-tis')),'--version'],check=True,capture_output=True)
commands={}
for variant in ['before','after']:
 for tier in ['bytecode','native']:commands[variant+'-'+tier]=[str(b/'binaries'/(variant+'-tis')),'--tier',tier,'--stats',str(b/'sieve_photo.py')]
commands.update({'cpython':[py,str(b/'sieve_photo.py')],'pypy':[pypy,str(b/'sieve_photo.py')]})
print('photo sieve 30M',flush=True);photo=measure(commands,'1857859\n');(b/'photo.json').write_text(json.dumps(photo,indent=2)+'\n')
old=json.loads(Path('/Users/jaheba/p/tis/benchmarks/results/2026-10-30-bytecode-builtins/oneshot.json').read_text());out={};(b/'workloads').mkdir(exist_ok=True)
for name,data in old.items():
 target=b/'workloads'/Path(data['commands']['cpython'][-1]).name;shutil.copy2(Path('/Users/jaheba/p/tis/benchmarks/results/2026-10-30-bytecode-builtins/workloads')/target.name,target);commands={}
 for variant in ['before','after']:
  for tier in ['bytecode','native']:
   command=[str(b/'binaries'/(variant+'-tis')),'--tier',tier]
   if name=='barnes-hut' and tier=='native':command+=['--readonly-regions','--mutation-regions']
   commands[variant+'-'+tier]=command+[str(target)]
 print(name,flush=True);out[name]=measure(commands,data['expected_stdout']);(b/'regressions.json').write_text(json.dumps(out,indent=2)+'\n')
