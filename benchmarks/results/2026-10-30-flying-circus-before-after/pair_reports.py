from pathlib import Path
import json,sys,copy
sys.path.insert(0,'/private/tmp/flying-circus-tis/src')
from flying_circus import export
base=Path('/private/tmp/tis-full-before-after');raw=base/'quick'
index=json.loads((raw/'run.json').read_text());startup=json.loads((raw/'startup.json').read_text());prov=json.loads((base/'provenance.json').read_text())
for tier in ['bytecode','native']:
 out=base/(tier+'-comparison');out.mkdir(exist_ok=False)
 keys=['before-'+tier,'after-'+tier,'monty','cpython','pypy'];view=copy.deepcopy(index)
 view['runtimes']={key:view['runtimes'][key] for key in keys};view['options']['output']=str(out)
 for phase in ['before','after']:
  key=phase+'-'+tier;view['runtimes'][key]['revision']=prov[phase+'_revision'];view['runtimes'][key]['build_info']='rustc1.99.0; release opt3, no LTO,16 codegen units; identical benchmark-only worker'
 (out/'run.json').write_text(json.dumps(view,indent=2)+'\n')
 start=copy.deepcopy(startup);start['engines']={key:start['engines'][key] for key in keys};(out/'startup.json').write_text(json.dumps(start,indent=2)+'\n')
 for mode in ['one-shot','repeated']:
  (out/mode).mkdir()
  for key in keys:
   d=json.loads((raw/mode/(key+'.json')).read_text())
   if key.startswith(('before-','after-')):d['revision']=view['runtimes'][key]['revision'];d['build_info']=view['runtimes'][key]['build_info']
   (out/mode/(key+'.json')).write_text(json.dumps(d,indent=2)+'\n')
 export.write_reports(out,['html','markdown','json'],False,5,.01)
print('Generated paired bytecode/native reports from original samples, without rerunning workloads.')
