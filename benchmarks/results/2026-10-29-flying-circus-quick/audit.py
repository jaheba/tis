import json,hashlib,time,sys
from pathlib import Path
sys.path.insert(0,'/private/tmp/flying-circus-tis/src')
from flying_circus.workers import TisWorker,WorkloadError
suite=Path('/Users/jaheba/p/flying-circus/src/flying_circus/workloads');manifest=json.loads((suite/'manifest.json').read_text());binary=Path('/private/tmp/tis-profile/circus')
report={'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'manifest_sha256':hashlib.sha256((suite/'manifest.json').read_bytes()).hexdigest(),'results':[]}
for tier,extra in [('native',['--readonly-regions','--mutation-regions'])]:
 for name,spec in manifest.items():
  source=suite/spec['file'];t=time.monotonic();row={'name':name,'tier':tier,'source_sha256':hashlib.sha256(source.read_bytes()).hexdigest()}
  try:
   with TisWorker(binary,30,arguments=['--tier',tier,*extra,'--diagnostics']) as w:
    w.ready();w.configure(source.name)
    r=w.request({'kind':'run','code':source.read_text(),'filename':source.name})
    row.update(status='pass' if r['stdout']==spec['stdout'] and not r['stderr'] else 'mismatch',stdout=r['stdout'],stats=r.get('stats'))
  except WorkloadError as e:row.update(status='unsupported',exception_type=e.exc_type,error=str(e))
  except Exception as e:row.update(status='error',error=str(e))
  row['seconds']=time.monotonic()-t;report['results'].append(row)
  Path('/private/tmp/tis-circus-compatibility.json').write_text(json.dumps(report,indent=2)+'\n')
  print(name,row['status'],flush=True)
print('Passed',sum(r['status']=='pass' for r in report['results']),len(report['results']),flush=True)
