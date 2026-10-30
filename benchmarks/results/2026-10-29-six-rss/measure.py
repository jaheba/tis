from pathlib import Path
import subprocess,json,re,hashlib
root=Path('/Users/jaheba/p/tis')
source=root/'benchmarks/results/2026-10-29-flying-circus-quick/quick/workloads'
binary=Path('/Users/jaheba/.cargo/bin/tis')
rows=[]
for name in ['game_nbody','perf_barnes_hut','game_spectral_norm','perf_float','perf_unpack_sequence','rustpython_micro_strings']:
 expected=None
 row={'name':name,'source_sha256':hashlib.sha256((source/(name+'.py')).read_bytes()).hexdigest()}
 for tier in ['bytecode','native']:
  flags=['--readonly-regions','--mutation-regions'] if tier=='native' else []
  run=subprocess.run(['/usr/bin/time','-l',str(binary),'--tier',tier,*flags,str(source/(name+'.py'))],capture_output=True,text=True,check=True)
  if expected is None:expected=run.stdout
  assert run.stdout==expected
  rss=int(re.search(r'(\d+)\s+maximum resident set size',run.stderr).group(1))
  row[tier]={'peak_rss_bytes':rss,'peak_rss_mib':rss/1048576}
 rows.append(row)
 print(name, row['bytecode']['peak_rss_mib'],row['native']['peak_rss_mib'],flush=True)
Path('/private/tmp/tis-six-rss.json').write_text(json.dumps({'binary':str(binary),'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'method':'One fresh process per tier/workload; macOS time -l whole-process peak RSS. Native enables readonly and mutation regions. Includes source/JIT compilation and execution. Current inline-string CLI.','results':rows},indent=2)+'\n')
