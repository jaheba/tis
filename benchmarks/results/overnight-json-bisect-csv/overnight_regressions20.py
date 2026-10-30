import subprocess,json,time
from pathlib import Path
r=Path('/Users/jaheba/p/tis');suite=r/'benchmarks/results/2026-10-29-flying-circus-quick/quick/workloads'
binaries={'before':r/'benchmarks/results/overnight-json-bisect-csv/binaries/tis-start','after':Path('/private/tmp/tis-overnight-candidate20-cli')}
# Warm executable validation only; each timed payload still starts a fresh process/VM.
for binary in binaries.values():subprocess.run([str(binary),'--version'],capture_output=True,check=True)
rows=[]
for name in ['game_nbody','perf_barnes_hut','perf_unpack_sequence']:
 expected=None
 for tier in ['bytecode','native']:
  row={'name':name,'tier':tier}
  for label,binary in binaries.items():
   command=[str(binary),'--tier',tier]
   if tier=='native': command+=['--readonly-regions','--mutation-regions']
   start=time.perf_counter();p=subprocess.run([*command,str(suite/(name+'.py'))],capture_output=True,check=True)
   row[label+'_ms']=(time.perf_counter()-start)*1000
   if expected is None:expected=p.stdout
   assert p.stdout==expected,(label,name,tier,p.stdout)
  rows.append(row);print(row,flush=True)
Path('/private/tmp/overnight-regressions20.json').write_text(json.dumps(rows,indent=2)+'\n')
