from pathlib import Path
import subprocess,time,json
r=Path('/Users/jaheba/p/tis/benchmarks/results/overnight-json-bisect-csv');o=Path('/private/tmp/overnight-profile20');o.mkdir(exist_ok=True)
rows=[]
for name,tier in [('text_csv_aggregate_medium','bytecode'),('text_csv_cleanup_medium','native')]:
 target=o/(name+'-'+tier)
 with target.with_suffix('.json').open('w') as stdout,target.with_suffix('.stderr').open('w') as stderr:
  p=subprocess.Popen(['/private/tmp/tis-overnight-candidate20-retained',str(r/'workloads'/(name+'.py')),tier,str(Path('/private/tmp/overnight-retained20')/(name+'.stdout'))],stdout=stdout,stderr=stderr)
  time.sleep(.1)
  sample=subprocess.run(['/usr/bin/sample',str(p.pid),'1','1','-file',str(target.with_suffix('.sample.txt'))],capture_output=True,text=True)
  code=p.wait()
  rows.append({'name':name,'tier':tier,'execution_exit':code,'sample_exit':sample.returncode,'sample_stdout':sample.stdout,'sample_stderr':sample.stderr,'note':'Profiled retained harness; timings are perturbed and excluded from benchmark evidence.'})
(o/'runs.json').write_text(json.dumps(rows,indent=2)+'\n')
print(rows)
