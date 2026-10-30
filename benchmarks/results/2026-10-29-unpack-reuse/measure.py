from pathlib import Path
import json,subprocess,time,statistics,re,hashlib
source=Path('/Users/jaheba/p/tis/benchmarks/results/2026-10-29-flying-circus-quick/quick/workloads/perf_unpack_sequence.py')
before=Path('/Users/jaheba/.cargo/bin/tis');after=Path('/private/tmp/tis-leaf-isolated-target/release/tis')
commands={'before_native':[str(before),'--tier','native','--readonly-regions','--mutation-regions','--stats'], 'after_native':[str(after),'--tier','native','--readonly-regions','--mutation-regions','--stats'],'bytecode':[str(after),'--tier','bytecode'],'cpython':['/opt/homebrew/bin/python3']}
samples={k:[] for k in commands};expected=None
for i in range(3):
 for name in (list(commands) if i%2==0 else list(reversed(commands))):
  cmd=['/usr/bin/time','-l',*commands[name],str(source)]
  start=time.perf_counter_ns();p=subprocess.run(cmd,text=True,capture_output=True,check=True);ms=(time.perf_counter_ns()-start)/1e6
  if expected is None:expected=p.stdout
  assert p.stdout==expected,(name,p.stdout)
  row={'elapsed_ms':ms,'peak_rss_bytes':int(re.search(r'(\d+)\s+maximum resident set size',p.stderr).group(1))}
  for field in ['native_compile_ns','native_code_bytes','compilation_errors','deoptimizations','native_region_entries']:
   m=re.search(r'\b'+field+r': (\d+)',p.stderr)
   if m:row[field]=int(m.group(1))
  samples[name].append(row)
  print(name,row,flush=True)
report={'method':'Three interleaved fresh-process samples. Full unchanged benchmark. Includes startup, source/JIT compilation and execution. Native enables both region flags. No build/test overlap. Peak whole-process RSS measured with macOS time -l.','source':str(source),'source_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),'stdout':expected,'commands':commands,'binary_sha256':{k:hashlib.sha256(Path(v[0]).read_bytes()).hexdigest() for k,v in commands.items()},'samples':samples,'medians':{k:{field:statistics.median(row[field] for row in rows) for field in rows[0]} for k,rows in samples.items()}}
Path('/private/tmp/tis-unpack-results.json').write_text(json.dumps(report,indent=2)+'\n')
print('Medians:',report['medians'],flush=True)
