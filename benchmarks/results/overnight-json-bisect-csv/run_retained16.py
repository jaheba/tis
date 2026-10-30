import subprocess,json,hashlib
from pathlib import Path
root=Path('/Users/jaheba/p/tis/benchmarks/results/overnight-json-bisect-csv')
out=Path('/private/tmp/overnight-retained16');out.mkdir(exist_ok=False)
binary=Path('/private/tmp/tis-overnight-candidate16-retained')
python='/opt/homebrew/Cellar/python@3.15/3.15.0/Frameworks/Python.framework/Versions/3.15/bin/python3.15'
rows=[]
for source in sorted((root/'workloads').glob('*.py')):
 expected=subprocess.run([python,str(source)],capture_output=True,text=True,check=True).stdout
 reference=out/(source.stem+'.stdout');reference.write_text(expected)
 for tier in ['bytecode','native']:
  result=subprocess.run([str(binary),str(source),tier,str(reference)],capture_output=True,text=True)
  row=json.loads(result.stdout) if result.returncode==0 else {'source':str(source),'tier':tier,'error':result.stderr,'exit_code':result.returncode}
  rows.append(row);print(source.stem,tier,row.get('retained_median_ns',row.get('error')),flush=True)
(out/'results.json').write_text(json.dumps({'binary':str(binary),'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'reference':python,'source_sha256':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in (root/'workloads').glob('*.py')},'rows':rows},indent=2)+'\n')
